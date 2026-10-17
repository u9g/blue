//! The daemon's durable state in SQLite: the sessions it owns, an outbox of events the server
//! hasn't acknowledged yet, and the server commands it has already processed.
//!
//! Every event is written to the outbox before it is sent, and removed only once the server
//! acknowledges it, so a dropped connection or a daemon restart loses nothing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use blue_protocol::daemon::v1::DaemonEvent;
use blue_protocol::events::v1::{SessionState, session_event::Kind};
use prost::Message;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id                TEXT PRIMARY KEY,
    claude_session_id TEXT,
    cwd               TEXT,
    model             TEXT,
    state             INTEGER NOT NULL,
    last_seq          INTEGER NOT NULL DEFAULT 0,
    current_turn      TEXT
);
CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    event      BLOB NOT NULL,
    UNIQUE (session_id, seq)
);
CREATE TABLE IF NOT EXISTS processed_commands (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
";

/// How long processed command ids are remembered (the server only resends on reconnect).
const COMMAND_MEMORY_SECS: i64 = 7 * 24 * 60 * 60;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredSession {
    pub id: String,
    pub claude_session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub state: SessionState,
    pub last_seq: u64,
    pub current_turn: Option<String>,
}

pub struct OutboxRow {
    pub id: i64,
    pub event: DaemonEvent,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(SCHEMA).context("create daemon schema")?;
        conn.execute(
            "DELETE FROM processed_commands WHERE at < ?1",
            [now() - COMMAND_MEMORY_SECS],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn sessions(&self) -> anyhow::Result<Vec<StoredSession>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, claude_session_id, cwd, model, state, last_seq, current_turn
             FROM sessions ORDER BY rowid",
        )?;
        let rows = statement.query_map([], session_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn session(&self, id: &str) -> anyhow::Result<Option<StoredSession>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT id, claude_session_id, cwd, model, state, last_seq, current_turn
                 FROM sessions WHERE id = ?1",
                [id],
                session_from_row,
            )
            .optional()?)
    }

    /// Records a new session; no-op if it already exists.
    pub fn insert_session(&self, id: &str, model: Option<&str>) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO sessions (id, model, state) VALUES (?1, ?2, ?3)",
            params![id, model, SessionState::Starting as i32],
        )?;
        Ok(())
    }

    pub fn command_seen(&self, command_id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT 1 FROM processed_commands WHERE id = ?1",
                [command_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn record_command(&self, command_id: &str) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO processed_commands (id, at) VALUES (?1, ?2)",
            params![command_id, now()],
        )?;
        Ok(())
    }

    /// Marks the command processed and the turn in progress, together: after a crash, a turn
    /// the daemon accepted is always known to have been interrupted.
    pub fn begin_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        command_id: &str,
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO processed_commands (id, at) VALUES (?1, ?2)",
            params![command_id, now()],
        )?;
        tx.execute(
            "UPDATE sessions SET current_turn = ?2 WHERE id = ?1",
            params![session_id, turn_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Undoes [`begin_turn`](Self::begin_turn) when the bridge refused the prompt.
    pub fn abort_turn(&self, session_id: &str, command_id: &str) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM processed_commands WHERE id = ?1", [command_id])?;
        tx.execute(
            "UPDATE sessions SET current_turn = NULL WHERE id = ?1",
            [session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Assigns the session's next seq to `event`, writes it to the outbox, and updates the
    /// session from it. Returns the event as stored.
    pub fn append(&self, mut event: DaemonEvent) -> anyhow::Result<DaemonEvent> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let last_seq: i64 = tx
            .query_row(
                "SELECT last_seq FROM sessions WHERE id = ?1",
                [&event.session_id],
                |row| row.get(0),
            )
            .optional()?
            .with_context(|| format!("event for unknown session {}", event.session_id))?;
        event.seq = last_seq as u64 + 1;
        tx.execute(
            "INSERT INTO outbox (session_id, seq, event) VALUES (?1, ?2, ?3)",
            params![event.session_id, event.seq as i64, event.encode_to_vec()],
        )?;
        tx.execute(
            "UPDATE sessions SET last_seq = ?2 WHERE id = ?1",
            params![event.session_id, event.seq as i64],
        )?;
        match event.event.as_ref().and_then(|event| event.kind.as_ref()) {
            Some(Kind::SessionStarted(started)) => {
                tx.execute(
                    "UPDATE sessions SET claude_session_id = ?2, cwd = ?3 WHERE id = ?1",
                    params![event.session_id, started.claude_session_id, started.cwd],
                )?;
            }
            Some(Kind::SessionStateChanged(changed)) => {
                tx.execute(
                    "UPDATE sessions SET state = ?2 WHERE id = ?1",
                    params![event.session_id, changed.state],
                )?;
            }
            Some(kind) if kind.ends_turn() => {
                tx.execute(
                    "UPDATE sessions SET current_turn = NULL WHERE id = ?1",
                    [&event.session_id],
                )?;
            }
            _ => {}
        }
        tx.commit()?;
        Ok(event)
    }

    /// Outbox events after row `after`, oldest first.
    pub fn outbox_after(&self, after: i64, limit: usize) -> anyhow::Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT id, event FROM outbox WHERE id > ?1 ORDER BY id LIMIT ?2")?;
        let rows = statement.query_map(params![after, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (id, bytes) = row?;
            Ok(OutboxRow {
                id,
                event: DaemonEvent::decode(bytes.as_slice()).context("decode outbox event")?,
            })
        })
        .collect()
    }

    pub fn outbox_len(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT count(*) FROM outbox", [], |row| {
            row.get::<_, i64>(0)
        })? as u64)
    }

    /// The server has stored the session's events up to `up_to_seq`.
    pub fn ack(&self, session_id: &str, up_to_seq: u64) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM outbox WHERE session_id = ?1 AND seq <= ?2",
            params![session_id, up_to_seq as i64],
        )?;
        Ok(())
    }

    pub fn ack_all(&self, acked: &HashMap<String, u64>) -> anyhow::Result<()> {
        for (session_id, seq) in acked {
            self.ack(session_id, *seq)?;
        }
        Ok(())
    }
}

fn session_from_row(row: &rusqlite::Row) -> rusqlite::Result<StoredSession> {
    Ok(StoredSession {
        id: row.get(0)?,
        claude_session_id: row.get(1)?,
        cwd: row.get(2)?,
        model: row.get(3)?,
        state: SessionState::try_from(row.get::<_, i32>(4)?).unwrap_or_default(),
        last_seq: row.get::<_, i64>(5)? as u64,
        current_turn: row.get(6)?,
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use blue_protocol::events::v1::{SessionEvent, SessionStarted, TurnCompleted, TurnStarted};

    use super::*;

    fn event(session: &str, kind: Kind) -> DaemonEvent {
        DaemonEvent {
            session_id: session.into(),
            seq: 0,
            turn_id: "t1".into(),
            occurred_at: None,
            event: Some(SessionEvent { kind: Some(kind) }),
        }
    }

    #[test]
    fn outbox_numbers_tracks_and_trims() {
        let store = Store::open_in_memory().unwrap();
        store.insert_session("s1", None).unwrap();
        store.begin_turn("s1", "t1", "prompt:t1").unwrap();
        assert!(store.command_seen("prompt:t1").unwrap());

        let first = store
            .append(event(
                "s1",
                Kind::TurnStarted(TurnStarted {
                    turn_id: "t1".into(),
                }),
            ))
            .unwrap();
        assert_eq!(first.seq, 1);
        store
            .append(event(
                "s1",
                Kind::SessionStarted(SessionStarted {
                    claude_session_id: "c1".into(),
                    model: "m".into(),
                    cwd: "/repo".into(),
                }),
            ))
            .unwrap();
        let session = store.session("s1").unwrap().unwrap();
        assert_eq!(session.current_turn.as_deref(), Some("t1"));
        assert_eq!(session.cwd.as_deref(), Some("/repo"));

        let done = store
            .append(event(
                "s1",
                Kind::TurnCompleted(TurnCompleted {
                    turn_id: "t1".into(),
                    ..Default::default()
                }),
            ))
            .unwrap();
        assert_eq!(done.seq, 3);
        assert_eq!(store.session("s1").unwrap().unwrap().current_turn, None);

        let rows = store.outbox_after(0, 10).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(store.outbox_after(rows[0].id, 10).unwrap().len(), 2);

        store.ack("s1", 2).unwrap();
        assert_eq!(store.outbox_len().unwrap(), 1);
        assert!(
            store
                .append(event("nope", Kind::TurnStarted(TurnStarted::default())))
                .is_err()
        );
    }

    #[test]
    fn survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store.insert_session("s1", Some("haiku")).unwrap();
            store
                .append(event("s1", Kind::TurnStarted(TurnStarted::default())))
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].last_seq, 1);
        assert_eq!(sessions[0].model.as_deref(), Some("haiku"));
        assert_eq!(store.outbox_len().unwrap(), 1);
    }
}
