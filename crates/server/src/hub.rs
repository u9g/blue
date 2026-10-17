//! The server's moving parts: live daemon connections, the event log (store + fan-out), and
//! dispatching queued turns to daemons.
//!
//! Every event insert goes through one lock, so event ids (the global cursor) become visible in
//! order and a subscriber that has read up to cursor N can never miss an event below N.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use blue_protocol::client::v1::{Event, MachineStatus, SessionEventEnvelope, event};
use blue_protocol::daemon::v1::{
    AttachResponse, Command, DaemonEvent, Hello, SendPrompt, StartSession, attach_response, command,
};
use blue_protocol::events::v1::{
    SessionEvent, SessionState, SessionStateChanged, TurnFailed, session_event::Kind,
};
use chrono::{DateTime, Utc};
use prost::Message;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use uuid::Uuid;

use crate::auth::internal;
use crate::time::{from_timestamp, timestamp};

pub type Outbound = mpsc::Sender<Result<AttachResponse, Status>>;

pub const CLOSED: i16 = SessionState::Closed as i16;
pub const FAILED: i16 = SessionState::Failed as i16;

struct Conn {
    id: u64,
    tx: Outbound,
    cancel: CancellationToken,
}

/// A daemon's registration in the hub; dropping the connection is up to the caller.
pub struct Attached {
    pub conn_id: u64,
    pub cancel: CancellationToken,
    /// Whether the machine already had a connection (now replaced).
    pub was_online: bool,
}

pub struct Hub {
    db: PgPool,
    conns: Mutex<HashMap<Uuid, Conn>>,
    next_conn: AtomicU64,
    write: tokio::sync::Mutex<()>,
    dispatch: tokio::sync::Mutex<()>,
    cursor: watch::Sender<i64>,
}

struct NewEvent {
    session_id: Option<Uuid>,
    turn_id: Option<Uuid>,
    seq: Option<i64>,
    kind: &'static str,
    payload: Vec<u8>,
    is_raw: bool,
    occurred_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
pub struct EventRow {
    pub id: i64,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub kind: String,
    pub payload: Vec<u8>,
    pub occurred_at: DateTime<Utc>,
}

impl EventRow {
    pub const COLUMNS: &str = "id, session_id, turn_id, kind, payload, occurred_at";

    pub fn into_event(self) -> Option<Event> {
        let kind = if self.kind == "machine_status" {
            event::Kind::MachineStatus(MachineStatus::decode(self.payload.as_slice()).ok()?)
        } else {
            event::Kind::Session(SessionEventEnvelope {
                session_id: self.session_id?.to_string(),
                turn_id: self.turn_id.map(|id| id.to_string()).unwrap_or_default(),
                event: Some(SessionEvent::decode(self.payload.as_slice()).ok()?),
            })
        };
        Some(Event {
            cursor: self.id as u64,
            occurred_at: Some(timestamp(self.occurred_at)),
            kind: Some(kind),
        })
    }
}

impl Hub {
    pub async fn new(db: PgPool) -> anyhow::Result<Self> {
        let latest: Option<i64> = sqlx::query_scalar("SELECT max(id) FROM events")
            .fetch_one(&db)
            .await?;
        Ok(Self {
            db,
            conns: Mutex::default(),
            next_conn: AtomicU64::new(1),
            write: tokio::sync::Mutex::new(()),
            dispatch: tokio::sync::Mutex::new(()),
            cursor: watch::Sender::new(latest.unwrap_or(0)),
        })
    }

    // ---- connections ----

    /// Registers a daemon's stream, replacing (and cancelling) any older one for the machine.
    pub fn connect(&self, machine_id: Uuid, tx: Outbound) -> Attached {
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let old = self.conns.lock().unwrap().insert(
            machine_id,
            Conn {
                id: conn_id,
                tx,
                cancel: cancel.clone(),
            },
        );
        let was_online = old.is_some();
        if let Some(old) = old {
            old.cancel.cancel();
        }
        Attached {
            conn_id,
            cancel,
            was_online,
        }
    }

    /// Removes the connection if it is still the current one; returns whether it was.
    pub fn disconnect(&self, machine_id: Uuid, conn_id: u64) -> bool {
        let mut conns = self.conns.lock().unwrap();
        if conns
            .get(&machine_id)
            .is_some_and(|conn| conn.id == conn_id)
        {
            conns.remove(&machine_id);
            true
        } else {
            false
        }
    }

    /// Drops a machine's stream, e.g. after its token was revoked.
    pub fn kick(&self, machine_id: Uuid) {
        if let Some(conn) = self.conns.lock().unwrap().get(&machine_id) {
            conn.cancel.cancel();
        }
    }

    pub fn is_online(&self, machine_id: Uuid) -> bool {
        self.conns.lock().unwrap().contains_key(&machine_id)
    }

    fn sender(&self, machine_id: Uuid) -> Option<Outbound> {
        self.conns
            .lock()
            .unwrap()
            .get(&machine_id)
            .map(|conn| conn.tx.clone())
    }

    // ---- event log ----

    /// Changes whenever new events are stored; holds the latest cursor.
    pub fn watch_cursor(&self) -> watch::Receiver<i64> {
        self.cursor.subscribe()
    }

    pub fn latest_cursor(&self) -> i64 {
        *self.cursor.borrow()
    }

    pub async fn machine_status(
        &self,
        machine_id: Uuid,
        machine_name: &str,
        online: bool,
    ) -> Result<(), Status> {
        let status = MachineStatus {
            machine_id: machine_id.to_string(),
            machine_name: machine_name.to_owned(),
            online,
        };
        let _guard = self.write.lock().await;
        let mut tx = self.db.begin().await.map_err(internal)?;
        let id = insert_event(
            &mut tx,
            NewEvent {
                session_id: None,
                turn_id: None,
                seq: None,
                kind: "machine_status",
                payload: status.encode_to_vec(),
                is_raw: false,
                occurred_at: Utc::now(),
            },
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.published(id);
        Ok(())
    }

    /// Stores one event from `machine_id`'s daemon and applies it to sessions and turns.
    /// Duplicates (already stored) and events for sessions the machine doesn't own are ignored.
    /// Returns whether a turn ended, which may free the session for its next turn.
    pub async fn append_daemon_event(
        &self,
        machine_id: Uuid,
        event: DaemonEvent,
    ) -> Result<bool, Status> {
        let Ok(session_id) = Uuid::parse_str(&event.session_id) else {
            tracing::warn!(session = %event.session_id, "event for a malformed session id");
            return Ok(false);
        };
        let owner: Option<Uuid> =
            sqlx::query_scalar("SELECT machine_id FROM sessions WHERE id = $1")
                .bind(session_id)
                .fetch_optional(&self.db)
                .await
                .map_err(internal)?;
        if owner != Some(machine_id) {
            tracing::warn!(%session_id, %machine_id, "event for a session this machine doesn't own");
            return Ok(false);
        }
        let turn_id = Uuid::parse_str(&event.turn_id).ok();
        let occurred_at = from_timestamp(event.occurred_at);
        let seq = event.seq as i64;
        let Some(session_event) = event.event else {
            return Ok(false);
        };
        let Some(kind) = &session_event.kind else {
            return Ok(false);
        };

        let _guard = self.write.lock().await;
        let mut tx = self.db.begin().await.map_err(internal)?;
        let id = insert_event(
            &mut tx,
            NewEvent {
                session_id: Some(session_id),
                turn_id,
                seq: Some(seq),
                kind: kind_name(kind),
                payload: session_event.encode_to_vec(),
                is_raw: matches!(kind, Kind::RawSdkMessage(_)),
                occurred_at,
            },
        )
        .await?;
        let mut turn_ended = false;
        if id.is_some() {
            turn_ended = apply(&mut tx, session_id, kind, occurred_at).await?;
            sqlx::query(
                "UPDATE sessions SET delivered = true, last_activity_at = greatest(last_activity_at, $2), updated_at = now() WHERE id = $1",
            )
            .bind(session_id)
            .bind(occurred_at)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        tx.commit().await.map_err(internal)?;
        self.published(id);
        Ok(turn_ended)
    }

    /// Fails a turn that hasn't finished, recording a TurnFailed event for it.
    pub async fn fail_turn(&self, turn_id: Uuid, reason: &str) -> Result<(), Status> {
        let _guard = self.write.lock().await;
        let mut tx = self.db.begin().await.map_err(internal)?;
        let id = fail_turn(&mut tx, turn_id, reason).await?;
        tx.commit().await.map_err(internal)?;
        self.published(id);
        Ok(())
    }

    /// The daemon no longer has this session: fail its unfinished turns and the session.
    pub async fn lose_session(&self, session_id: Uuid, reason: &str) -> Result<(), Status> {
        let _guard = self.write.lock().await;
        let mut tx = self.db.begin().await.map_err(internal)?;
        let turns: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM turns WHERE session_id = $1 AND state IN ('queued', 'running') ORDER BY index",
        )
        .bind(session_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        let mut last = None;
        for turn_id in turns {
            last = fail_turn(&mut tx, turn_id, reason).await?.or(last);
        }
        let kind = Kind::SessionStateChanged(SessionStateChanged {
            state: SessionState::Failed.into(),
        });
        let now = Utc::now();
        last = insert_event(&mut tx, server_session_event(session_id, None, &kind, now))
            .await?
            .or(last);
        apply(&mut tx, session_id, &kind, now).await?;
        tx.commit().await.map_err(internal)?;
        self.published(last);
        Ok(())
    }

    fn published(&self, id: Option<i64>) {
        if let Some(id) = id {
            self.cursor.send_if_modified(|latest| {
                let newer = id > *latest;
                if newer {
                    *latest = id;
                }
                newer
            });
        }
    }

    // ---- daemons ----

    /// Reconciles after a daemon (re)connects: sessions the server handed to it that it no
    /// longer knows are failed. Returns, for the sessions it does know, the highest stored seq.
    pub async fn reconcile(
        &self,
        machine_id: Uuid,
        hello: &Hello,
    ) -> Result<HashMap<String, u64>, Status> {
        let known: HashSet<Uuid> = hello
            .sessions
            .iter()
            .filter_map(|session| Uuid::parse_str(&session.session_id).ok())
            .collect();
        let known_ids: Vec<Uuid> = known.iter().copied().collect();

        let open: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE machine_id = $1 AND delivered AND state NOT IN ($2, $3)",
        )
        .bind(machine_id)
        .bind(CLOSED)
        .bind(FAILED)
        .fetch_all(&self.db)
        .await
        .map_err(internal)?;
        for session_id in open.into_iter().filter(|id| !known.contains(id)) {
            tracing::warn!(%session_id, "daemon lost session");
            self.lose_session(session_id, "daemon lost session").await?;
        }

        sqlx::query(
            "UPDATE sessions SET delivered = true WHERE machine_id = $1 AND id = ANY($2) AND NOT delivered",
        )
        .bind(machine_id)
        .bind(&known_ids)
        .execute(&self.db)
        .await
        .map_err(internal)?;

        let acked: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT e.session_id, max(e.seq) FROM events e JOIN sessions s ON s.id = e.session_id
             WHERE s.machine_id = $1 AND e.session_id = ANY($2) AND e.seq IS NOT NULL
             GROUP BY e.session_id",
        )
        .bind(machine_id)
        .bind(&known_ids)
        .fetch_all(&self.db)
        .await
        .map_err(internal)?;
        Ok(acked
            .into_iter()
            .map(|(session_id, seq)| (session_id.to_string(), seq as u64))
            .collect())
    }

    pub async fn mark_delivered(&self, session_id: Uuid) -> Result<(), Status> {
        sqlx::query("UPDATE sessions SET delivered = true WHERE id = $1")
            .bind(session_id)
            .execute(&self.db)
            .await
            .map_err(internal)?;
        Ok(())
    }

    /// Sends the machine's daemon whatever it should be doing: StartSession for sessions it
    /// doesn't have yet, and the next queued turn for each session with no turn in flight.
    /// With `resend`, turns sent before but not yet started are sent again (after a reconnect;
    /// the daemon ignores commands it has already processed). No-op if the machine is offline.
    pub async fn dispatch(&self, machine_id: Uuid, resend: bool) -> Result<(), Status> {
        let Some(tx) = self.sender(machine_id) else {
            return Ok(());
        };
        let _guard = self.dispatch.lock().await;
        let sessions: Vec<(Uuid, bool)> = sqlx::query_as(
            "SELECT s.id, s.delivered FROM sessions s
             WHERE s.machine_id = $1 AND s.state <> $2
               AND (NOT s.delivered OR EXISTS (
                   SELECT 1 FROM turns t WHERE t.session_id = s.id AND t.state = 'queued'))
             ORDER BY s.created_at",
        )
        .bind(machine_id)
        .bind(CLOSED)
        .fetch_all(&self.db)
        .await
        .map_err(internal)?;

        for (session_id, delivered) in sessions {
            let mut commands = Vec::new();
            if !delivered {
                commands.push(Command {
                    command_id: format!("start:{session_id}"),
                    command: Some(command::Command::StartSession(StartSession {
                        session_id: session_id.to_string(),
                        resume_claude_session_id: None,
                        model: None,
                    })),
                });
            }

            let in_flight: Option<(Uuid, String, String)> = sqlx::query_as(
                "SELECT id, instructions, state FROM turns
                 WHERE session_id = $1
                   AND (state = 'running' OR (state = 'queued' AND dispatched_at IS NOT NULL))
                 ORDER BY index LIMIT 1",
            )
            .bind(session_id)
            .fetch_optional(&self.db)
            .await
            .map_err(internal)?;
            let next = match in_flight {
                Some((turn_id, text, state)) if state == "queued" && resend => {
                    Some((turn_id, text))
                }
                Some(_) => None,
                None => sqlx::query_as(
                    "UPDATE turns SET dispatched_at = now()
                     WHERE id = (SELECT id FROM turns WHERE session_id = $1 AND state = 'queued'
                                 ORDER BY index LIMIT 1)
                     RETURNING id, instructions",
                )
                .bind(session_id)
                .fetch_optional(&self.db)
                .await
                .map_err(internal)?,
            };
            if let Some((turn_id, text)) = next {
                commands.push(Command {
                    command_id: format!("prompt:{turn_id}"),
                    command: Some(command::Command::SendPrompt(SendPrompt {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        text,
                    })),
                });
            }

            for command in commands {
                let message = AttachResponse {
                    message: Some(attach_response::Message::Command(command)),
                };
                if tx.send(Ok(message)).await.is_err() {
                    // Disconnected; the next connection resends.
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// Inserts an event; `None` if it was a duplicate.
async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    event: NewEvent,
) -> Result<Option<i64>, Status> {
    sqlx::query_scalar(
        "INSERT INTO events (session_id, turn_id, seq, kind, payload, is_raw, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (session_id, seq) DO NOTHING
         RETURNING id",
    )
    .bind(event.session_id)
    .bind(event.turn_id)
    .bind(event.seq)
    .bind(event.kind)
    .bind(event.payload)
    .bind(event.is_raw)
    .bind(event.occurred_at)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)
}

fn server_session_event(
    session_id: Uuid,
    turn_id: Option<Uuid>,
    kind: &Kind,
    at: DateTime<Utc>,
) -> NewEvent {
    NewEvent {
        session_id: Some(session_id),
        turn_id,
        seq: None,
        kind: kind_name(kind),
        payload: SessionEvent {
            kind: Some(kind.clone()),
        }
        .encode_to_vec(),
        is_raw: false,
        occurred_at: at,
    }
}

async fn fail_turn(
    tx: &mut Transaction<'_, Postgres>,
    turn_id: Uuid,
    reason: &str,
) -> Result<Option<i64>, Status> {
    let session_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT session_id FROM turns WHERE id = $1 AND state IN ('queued', 'running')",
    )
    .bind(turn_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let kind = Kind::TurnFailed(TurnFailed {
        turn_id: turn_id.to_string(),
        reason: reason.to_owned(),
    });
    let now = Utc::now();
    let id = insert_event(
        tx,
        server_session_event(session_id, Some(turn_id), &kind, now),
    )
    .await?;
    apply(tx, session_id, &kind, now).await?;
    Ok(id)
}

/// Applies an event to the session and turn tables. Returns whether a turn ended.
async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    session_id: Uuid,
    kind: &Kind,
    at: DateTime<Utc>,
) -> Result<bool, Status> {
    let turn = |id: &str| Uuid::parse_str(id).ok();
    let result = match kind {
        Kind::SessionStarted(started) => {
            sqlx::query("UPDATE sessions SET claude_session_id = $2 WHERE id = $1")
                .bind(session_id)
                .bind(&started.claude_session_id)
                .execute(&mut **tx)
                .await
        }
        Kind::SessionStateChanged(changed) => {
            sqlx::query("UPDATE sessions SET state = $2, updated_at = now() WHERE id = $1")
                .bind(session_id)
                .bind(changed.state as i16)
                .execute(&mut **tx)
                .await
        }
        Kind::TurnStarted(started) => {
            sqlx::query(
                "UPDATE turns SET state = 'running', started_at = $3
                 WHERE id = $2 AND session_id = $1 AND state = 'queued'",
            )
            .bind(session_id)
            .bind(turn(&started.turn_id))
            .bind(at)
            .execute(&mut **tx)
            .await
        }
        Kind::TurnCompleted(done) => {
            sqlx::query(
                "UPDATE turns SET state = 'completed', result_text = $3, duration_ms = $4, finished_at = $5
                 WHERE id = $2 AND session_id = $1 AND state IN ('queued', 'running')",
            )
            .bind(session_id)
            .bind(turn(&done.turn_id))
            .bind(&done.result_text)
            .bind(done.duration_ms as i64)
            .bind(at)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
            return Ok(true);
        }
        Kind::TurnFailed(failed) => {
            sqlx::query(
                "UPDATE turns SET state = 'failed', error = $3, finished_at = $4
                 WHERE id = $2 AND session_id = $1 AND state IN ('queued', 'running')",
            )
            .bind(session_id)
            .bind(turn(&failed.turn_id))
            .bind(&failed.reason)
            .bind(at)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
            return Ok(true);
        }
        _ => return Ok(false),
    };
    result.map_err(internal)?;
    Ok(false)
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::SessionStarted(_) => "session_started",
        Kind::TurnStarted(_) => "turn_started",
        Kind::AssistantText(_) => "assistant_text",
        Kind::ToolStarted(_) => "tool_started",
        Kind::ToolFinished(_) => "tool_finished",
        Kind::TurnCompleted(_) => "turn_completed",
        Kind::TurnFailed(_) => "turn_failed",
        Kind::SessionStateChanged(_) => "session_state_changed",
        Kind::RawSdkMessage(_) => "raw_sdk_message",
    }
}
