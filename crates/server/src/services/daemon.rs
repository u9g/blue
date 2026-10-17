//! DaemonService: one long-lived Attach stream per machine.

use std::pin::Pin;
use std::sync::Arc;

use blue_protocol::PROTOCOL_VERSION;
use blue_protocol::daemon::v1::daemon_service_server::DaemonService;
use blue_protocol::daemon::v1::{
    AttachRequest, AttachResponse, CommandResult, EventsAck, GetMachineRequest, GetMachineResponse,
    Hello, Welcome, attach_request, attach_response,
};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{MachinePrincipal, internal};
use crate::hub::Outbound;

pub struct Daemons(pub Arc<AppState>);

type AttachStream = Pin<Box<dyn Stream<Item = Result<AttachResponse, Status>> + Send>>;

#[tonic::async_trait]
impl DaemonService for Daemons {
    type AttachStream = AttachStream;

    async fn attach(
        &self,
        request: Request<Streaming<AttachRequest>>,
    ) -> Result<Response<AttachStream>, Status> {
        let state = self.0.clone();
        let machine = state.auth.machine(request.metadata()).await?;
        let mut inbound = request.into_inner();
        let hello = match inbound.message().await? {
            Some(AttachRequest {
                message: Some(attach_request::Message::Hello(hello)),
            }) => hello,
            _ => return Err(Status::invalid_argument("the first message must be Hello")),
        };
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(Status::failed_precondition(format!(
                "daemon speaks protocol v{}, server speaks v{PROTOCOL_VERSION}",
                hello.protocol_version
            )));
        }

        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let attached = state.hub.connect(machine.id, tx.clone());
            tracing::info!(machine = %machine.name, "daemon attached");
            if !attached.was_online
                && let Err(err) = state
                    .hub
                    .machine_status(machine.id, &machine.name, true)
                    .await
            {
                tracing::error!("record machine online: {err}");
            }
            let result = tokio::select! {
                result = run(&state, &machine, hello, inbound, &tx) => result,
                () = attached.cancel.cancelled() => Ok(()),
                () = state.shutdown.cancelled() => Err(Status::unavailable("server shutting down")),
            };
            if let Err(status) = result {
                tracing::warn!(machine = %machine.name, "daemon stream ended: {}", status.message());
                let _ = tx.send(Err(status)).await;
            }
            if state.hub.disconnect(machine.id, attached.conn_id) {
                tracing::info!(machine = %machine.name, "daemon detached");
                let _ = sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
                    .bind(machine.id)
                    .execute(&state.db)
                    .await;
                if let Err(err) = state
                    .hub
                    .machine_status(machine.id, &machine.name, false)
                    .await
                {
                    tracing::error!("record machine offline: {err}");
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn get_machine(
        &self,
        request: Request<GetMachineRequest>,
    ) -> Result<Response<GetMachineResponse>, Status> {
        let machine = self.0.auth.machine(request.metadata()).await?;
        Ok(Response::new(GetMachineResponse {
            machine_id: machine.id.to_string(),
            machine_name: machine.name,
        }))
    }
}

async fn run(
    state: &AppState,
    machine: &MachinePrincipal,
    hello: Hello,
    mut inbound: Streaming<AttachRequest>,
    tx: &Outbound,
) -> Result<(), Status> {
    let versions = hello.versions.clone().unwrap_or_default();
    sqlx::query(
        "UPDATE machines SET os = $2, arch = $3, daemon_version = $4, bridge_version = $5,
             sdk_version = $6, claude_version = $7, last_seen_at = now()
         WHERE id = $1",
    )
    .bind(machine.id)
    .bind(&hello.os)
    .bind(&hello.arch)
    .bind(&versions.daemon)
    .bind(&versions.bridge)
    .bind(&versions.sdk)
    .bind(&versions.claude)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    let acked_seqs = state.hub.reconcile(machine.id, &hello).await?;
    send(
        tx,
        attach_response::Message::Welcome(Welcome {
            machine_id: machine.id.to_string(),
            acked_seqs,
        }),
    )
    .await?;
    state.hub.dispatch(machine.id, true).await?;

    while let Some(message) = inbound.message().await? {
        match message.message {
            Some(attach_request::Message::Event(event)) => {
                let session_id = event.session_id.clone();
                let seq = event.seq;
                let turn_ended = state.hub.append_daemon_event(machine.id, event).await?;
                send(
                    tx,
                    attach_response::Message::EventsAck(EventsAck {
                        session_id,
                        up_to_seq: seq,
                    }),
                )
                .await?;
                if turn_ended {
                    state.hub.dispatch(machine.id, false).await?;
                }
            }
            Some(attach_request::Message::CommandResult(result)) => {
                on_command_result(state, machine, result).await?;
            }
            Some(attach_request::Message::Hello(_)) => {
                return Err(Status::invalid_argument("Hello may only be sent first"));
            }
            None => {}
        }
    }
    Ok(())
}

async fn on_command_result(
    state: &AppState,
    machine: &MachinePrincipal,
    result: CommandResult,
) -> Result<(), Status> {
    let (verb, id) = result
        .command_id
        .split_once(':')
        .unwrap_or((&result.command_id, ""));
    let Ok(id) = Uuid::parse_str(id) else {
        return Ok(());
    };
    match (verb, result.ok) {
        ("start", true) => state.hub.mark_delivered(id).await?,
        ("prompt", false) => {
            tracing::warn!(machine = %machine.name, turn = %id, "daemon rejected turn: {}", result.error_message);
            let reason = format!(
                "the daemon could not start the turn: {}",
                result.error_message
            );
            state.hub.fail_turn(id, &reason).await?;
            state.hub.dispatch(machine.id, false).await?;
        }
        (_, false) => {
            tracing::warn!(machine = %machine.name, command = %result.command_id, "command failed: {}", result.error_message)
        }
        _ => {}
    }
    Ok(())
}

async fn send(tx: &Outbound, message: attach_response::Message) -> Result<(), Status> {
    tx.send(Ok(AttachResponse {
        message: Some(message),
    }))
    .await
    .map_err(|_| Status::cancelled("daemon disconnected"))
}
