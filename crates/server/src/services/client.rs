//! ClientService: what the voice agent and the CLI use.

use std::pin::Pin;
use std::sync::Arc;

use blue_protocol::client::v1::client_service_server::ClientService;
use blue_protocol::client::v1::{
    AmbiguousTarget, CreateSessionRequest, CreateSessionResponse, GetSessionRequest,
    GetSessionResponse, ListMachinesRequest, ListMachinesResponse, ListSessionsRequest,
    ListSessionsResponse, Machine, SendPromptRequest, SendPromptResponse, Session,
    SubscribeRequest, SubscribeResponse, Turn, TurnState,
};
use chrono::{DateTime, Utc};
use prost::Message;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{audit, internal};
use crate::hub::{CLOSED, EventRow, FAILED};
use crate::time::{opt_timestamp, timestamp};

pub struct Clients(pub Arc<AppState>);

type SubscribeStream = Pin<Box<dyn Stream<Item = Result<SubscribeResponse, Status>> + Send>>;

const SUBSCRIBE_BATCH: i64 = 500;

#[tonic::async_trait]
impl ClientService for Clients {
    type SubscribeStream = SubscribeStream;

    async fn list_machines(
        &self,
        request: Request<ListMachinesRequest>,
    ) -> Result<Response<ListMachinesResponse>, Status> {
        let state = &self.0;
        state.auth.client(request.metadata()).await?;
        let rows: Vec<(Uuid, String, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT id, name, os, last_seen_at FROM machines WHERE revoked_at IS NULL ORDER BY lower(name)",
        )
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
        let machines = rows
            .into_iter()
            .map(|(id, name, os, last_seen_at)| Machine {
                id: id.to_string(),
                name,
                online: state.hub.is_online(id),
                os,
                last_seen_at: opt_timestamp(last_seen_at),
            })
            .collect();
        Ok(Response::new(ListMachinesResponse { machines }))
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let state = &self.0;
        state.auth.client(request.metadata()).await?;
        let request = request.into_inner();
        let machine_id = if request.machine.is_empty() {
            None
        } else {
            Some(resolve_machine(state, &request.machine).await?.0)
        };
        let rows: Vec<SessionRow> = sqlx::query_as(&format!(
            "{SESSION_SELECT}
             WHERE ($1::uuid IS NULL OR s.machine_id = $1)
               AND (NOT $2 OR s.state NOT IN ($3, $4))
             ORDER BY s.last_activity_at DESC"
        ))
        .bind(machine_id)
        .bind(request.active_only)
        .bind(CLOSED)
        .bind(FAILED)
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
        Ok(Response::new(ListSessionsResponse {
            sessions: sessions_with_turns(state, rows).await?,
        }))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let state = &self.0;
        let client = state.auth.client(request.metadata()).await?;
        let request = request.into_inner();
        let title = request.title.trim();
        if title.is_empty() {
            return Err(Status::invalid_argument("title is required"));
        }
        if request.instructions.trim().is_empty() {
            return Err(Status::invalid_argument("instructions are required"));
        }
        let (machine_id, machine_name) = resolve_machine(state, &request.machine).await?;

        let session_id = Uuid::now_v7();
        let turn_id = Uuid::now_v7();
        let mut tx = state.db.begin().await.map_err(internal)?;
        sqlx::query("INSERT INTO sessions (id, machine_id, title, state) VALUES ($1, $2, $3, $4)")
            .bind(session_id)
            .bind(machine_id)
            .bind(title)
            .bind(blue_protocol::events::v1::SessionState::Starting as i16)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        insert_turn(&mut tx, turn_id, session_id, 0, &request.instructions).await?;
        tx.commit().await.map_err(internal)?;
        audit(
            &state.db,
            &client.actor(),
            "session.create",
            json!({ "session_id": session_id, "machine": machine_name, "title": title }),
        )
        .await;

        state.hub.dispatch(machine_id, false).await?;
        let session = load_session(state, session_id).await?;
        Ok(Response::new(CreateSessionResponse {
            session: Some(session),
            turn_id: turn_id.to_string(),
        }))
    }

    async fn send_prompt(
        &self,
        request: Request<SendPromptRequest>,
    ) -> Result<Response<SendPromptResponse>, Status> {
        let state = &self.0;
        state.auth.client(request.metadata()).await?;
        let request = request.into_inner();
        if request.instructions.trim().is_empty() {
            return Err(Status::invalid_argument("instructions are required"));
        }
        let session_id = parse_session_id(&request.session_id)?;

        let mut tx = state.db.begin().await.map_err(internal)?;
        // Locks the session so concurrent follow-ups get distinct indexes.
        let session: Option<(Uuid, i16)> =
            sqlx::query_as("SELECT machine_id, state FROM sessions WHERE id = $1 FOR UPDATE")
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?;
        let (machine_id, session_state) =
            session.ok_or_else(|| Status::not_found("no such session"))?;
        if session_state == CLOSED {
            return Err(Status::failed_precondition("the session is closed"));
        }
        let index: i32 = sqlx::query_scalar(
            "SELECT coalesce(max(index) + 1, 0) FROM turns WHERE session_id = $1",
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        let turn_id = Uuid::now_v7();
        insert_turn(&mut tx, turn_id, session_id, index, &request.instructions).await?;
        tx.commit().await.map_err(internal)?;

        state.hub.dispatch(machine_id, false).await?;
        Ok(Response::new(SendPromptResponse {
            turn_id: turn_id.to_string(),
        }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let state = &self.0;
        state.auth.client(request.metadata()).await?;
        let session_id = parse_session_id(&request.into_inner().session_id)?;
        let session = load_session(state, session_id).await?;
        let rows: Vec<EventRow> = sqlx::query_as(&format!(
            "SELECT {} FROM events WHERE session_id = $1 AND NOT is_raw ORDER BY id",
            EventRow::COLUMNS
        ))
        .bind(session_id)
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
        Ok(Response::new(GetSessionResponse {
            session: Some(session),
            events: rows.into_iter().filter_map(EventRow::into_event).collect(),
        }))
    }

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<SubscribeStream>, Status> {
        let state = self.0.clone();
        state.auth.client(request.metadata()).await?;
        let request = request.into_inner();
        let session_ids = if request.session_ids.is_empty() {
            None
        } else {
            Some(
                request
                    .session_ids
                    .iter()
                    .map(|id| parse_session_id(id))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };
        let mut watch = state.hub.watch_cursor();
        let mut cursor = match request.cursor {
            Some(cursor) => cursor as i64,
            None => state.hub.latest_cursor(),
        };

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                // Mark the latest cursor seen before querying, so anything stored after the
                // query wakes the wait below.
                watch.borrow_and_update();
                let rows: Result<Vec<EventRow>, _> = sqlx::query_as(&format!(
                    "SELECT {} FROM events
                     WHERE id > $1 AND ($2::uuid[] IS NULL OR session_id = ANY($2))
                       AND ($3 OR NOT is_raw)
                     ORDER BY id LIMIT $4",
                    EventRow::COLUMNS
                ))
                .bind(cursor)
                .bind(&session_ids)
                .bind(request.include_raw)
                .bind(SUBSCRIBE_BATCH)
                .fetch_all(&state.db)
                .await;
                let rows = match rows {
                    Ok(rows) => rows,
                    Err(err) => {
                        let _ = tx.send(Err(internal(err))).await;
                        return;
                    }
                };
                if rows.is_empty() {
                    tokio::select! {
                        changed = watch.changed() => if changed.is_err() { return },
                        () = tx.closed() => return,
                        () = state.shutdown.cancelled() => {
                            let _ = tx.send(Err(Status::unavailable("server shutting down"))).await;
                            return;
                        }
                    }
                    continue;
                }
                for row in rows {
                    cursor = row.id;
                    let Some(event) = row.into_event() else {
                        continue;
                    };
                    let response = SubscribeResponse { event: Some(event) };
                    if tx.send(Ok(response)).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

const SESSION_SELECT: &str = "SELECT s.id, s.machine_id, m.name AS machine_name, s.title, s.state,
        s.created_at, s.last_activity_at
     FROM sessions s JOIN machines m ON m.id = s.machine_id";

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: Uuid,
    machine_id: Uuid,
    machine_name: String,
    title: String,
    state: i16,
    created_at: DateTime<Utc>,
    last_activity_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct TurnRow {
    id: Uuid,
    session_id: Uuid,
    index: i32,
    instructions: String,
    state: String,
    result_text: String,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
}

impl From<TurnRow> for Turn {
    fn from(row: TurnRow) -> Self {
        let state = match row.state.as_str() {
            "queued" => TurnState::Queued,
            "running" => TurnState::Running,
            "completed" => TurnState::Completed,
            "failed" => TurnState::Failed,
            _ => TurnState::Unspecified,
        };
        Turn {
            id: row.id.to_string(),
            index: row.index as u32,
            instructions: row.instructions,
            state: state.into(),
            result_text: row.result_text,
            started_at: opt_timestamp(row.started_at),
            finished_at: opt_timestamp(row.finished_at),
        }
    }
}

/// Attaches each session's current turn: the running one, else the next queued one, else the
/// latest.
async fn sessions_with_turns(
    state: &AppState,
    rows: Vec<SessionRow>,
) -> Result<Vec<Session>, Status> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let turns: Vec<TurnRow> = sqlx::query_as(
        "SELECT DISTINCT ON (session_id) id, session_id, index, instructions, state, result_text,
                started_at, finished_at
         FROM turns WHERE session_id = ANY($1)
         ORDER BY session_id,
                  CASE state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END,
                  CASE WHEN state = 'queued' THEN index ELSE -index END",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    let mut turns: std::collections::HashMap<Uuid, TurnRow> = turns
        .into_iter()
        .map(|turn| (turn.session_id, turn))
        .collect();
    Ok(rows
        .into_iter()
        .map(|row| Session {
            id: row.id.to_string(),
            machine_id: row.machine_id.to_string(),
            machine_name: row.machine_name,
            title: row.title,
            state: row.state.into(),
            created_at: Some(timestamp(row.created_at)),
            last_activity_at: Some(timestamp(row.last_activity_at)),
            current_turn: turns.remove(&row.id).map(Turn::from),
        })
        .collect())
}

async fn load_session(state: &AppState, session_id: Uuid) -> Result<Session, Status> {
    let row: Option<SessionRow> = sqlx::query_as(&format!("{SESSION_SELECT} WHERE s.id = $1"))
        .bind(session_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;
    let row = row.ok_or_else(|| Status::not_found("no such session"))?;
    Ok(sessions_with_turns(state, vec![row]).await?.remove(0))
}

async fn insert_turn(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    turn_id: Uuid,
    session_id: Uuid,
    index: i32,
    instructions: &str,
) -> Result<(), Status> {
    sqlx::query(
        "INSERT INTO turns (id, session_id, index, instructions, state) VALUES ($1, $2, $3, $4, 'queued')",
    )
    .bind(turn_id)
    .bind(session_id)
    .bind(index)
    .bind(instructions)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    Ok(())
}

fn parse_session_id(id: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(id).map_err(|_| Status::not_found(format!("no session {id:?}")))
}

/// Finds a machine by id or name: an exact name (any case) wins, else a unique name prefix.
/// Several matches fail with FAILED_PRECONDITION and an [`AmbiguousTarget`] detail.
pub async fn resolve_machine(state: &AppState, query: &str) -> Result<(Uuid, String), Status> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Status::invalid_argument("machine is required"));
    }
    if let Ok(id) = Uuid::parse_str(query) {
        let row: Option<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM machines WHERE id = $1 AND revoked_at IS NULL")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .map_err(internal)?;
        return row.ok_or_else(|| Status::not_found(format!("no machine {query:?}")));
    }
    let pattern = format!(
        "{}%",
        query
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM machines
         WHERE revoked_at IS NULL AND lower(name) LIKE $1
         ORDER BY lower(name)",
    )
    .bind(&pattern)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    if let Some(exact) = rows
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(query))
    {
        return Ok(exact.clone());
    }
    match rows.len() {
        0 => Err(Status::not_found(format!("no machine named {query:?}"))),
        1 => Ok(rows.into_iter().next().unwrap()),
        _ => {
            let candidates: Vec<String> = rows.into_iter().map(|(_, name)| name).collect();
            Err(ambiguous(query, candidates))
        }
    }
}

fn ambiguous(query: &str, candidates: Vec<String>) -> Status {
    let message = format!(
        "{query:?} matches several machines: {}",
        candidates.join(", ")
    );
    let detail = AmbiguousTarget {
        query: query.to_owned(),
        candidates,
    };
    let status = RpcStatus {
        code: Code::FailedPrecondition as i32,
        message: message.clone(),
        details: vec![prost_types::Any {
            type_url: "type.googleapis.com/blue.client.v1.AmbiguousTarget".into(),
            value: detail.encode_to_vec(),
        }],
    };
    Status::with_details(
        Code::FailedPrecondition,
        message,
        status.encode_to_vec().into(),
    )
}

/// `google.rpc.Status`, the standard container for gRPC error details.
#[derive(Clone, PartialEq, Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}
