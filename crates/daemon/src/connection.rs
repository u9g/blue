//! The daemon's Attach stream to the server: introduce ourselves, replay the outbox, then
//! stream new events while handing commands to the core. Reconnects with jittered backoff
//! until the server rejects the machine token.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use blue_common::Bearer;
use blue_protocol::PROTOCOL_VERSION;
use blue_protocol::daemon::v1::daemon_service_client::DaemonServiceClient;
use blue_protocol::daemon::v1::{
    AttachRequest, AttachResponse, Command, CommandResult, Hello, SessionSnapshot, Versions,
    attach_request, attach_response,
};
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Code;

use crate::store::Store;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const OUTBOX_BATCH: usize = 256;

#[derive(Clone)]
pub struct ConnectionOptions {
    pub server_url: String,
    pub token: String,
    pub versions: Versions,
}

pub async fn run(
    options: ConnectionOptions,
    store: Arc<Store>,
    outbox: Arc<Notify>,
    commands: mpsc::Sender<Command>,
    mut results: mpsc::UnboundedReceiver<CommandResult>,
) -> anyhow::Result<()> {
    let mut backoff = MIN_BACKOFF;
    loop {
        let mut welcomed = false;
        match attach(
            &options,
            &store,
            &outbox,
            &commands,
            &mut results,
            &mut welcomed,
        )
        .await
        {
            Ok(()) => tracing::warn!("server closed the connection"),
            Err(err) => {
                if let Some(status) = err.downcast_ref::<tonic::Status>()
                    && matches!(
                        status.code(),
                        Code::Unauthenticated | Code::PermissionDenied | Code::FailedPrecondition
                    )
                {
                    return Err(err.context("the server refused this daemon"));
                }
                tracing::warn!("server connection: {err:#}");
            }
        }
        if welcomed {
            backoff = MIN_BACKOFF;
        }
        // Results for commands from the old connection: the server resends what it still needs.
        while results.try_recv().is_ok() {}
        let delay = jitter(backoff);
        tracing::info!("reconnecting in {delay:?}");
        tokio::time::sleep(delay).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn attach(
    options: &ConnectionOptions,
    store: &Store,
    outbox: &Notify,
    commands: &mpsc::Sender<Command>,
    results: &mut mpsc::UnboundedReceiver<CommandResult>,
    welcomed: &mut bool,
) -> anyhow::Result<()> {
    let channel = blue_common::connect(&options.server_url).await?;
    let mut client = DaemonServiceClient::with_interceptor(channel, Bearer::new(&options.token)?);

    let (tx, rx) = mpsc::channel(256);
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        versions: Some(options.versions.clone()),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        sessions: store
            .sessions()?
            .into_iter()
            .map(|session| SessionSnapshot {
                session_id: session.id,
                claude_session_id: session.claude_session_id.unwrap_or_default(),
                state: session.state.into(),
                last_seq: session.last_seq,
            })
            .collect(),
    };
    send(&tx, attach_request::Message::Hello(hello)).await?;
    let mut inbound = client.attach(ReceiverStream::new(rx)).await?.into_inner();

    let welcome = match inbound.message().await? {
        Some(AttachResponse {
            message: Some(attach_response::Message::Welcome(welcome)),
        }) => welcome,
        other => bail!("expected Welcome, got {other:?}"),
    };
    store.ack_all(&welcome.acked_seqs)?;
    *welcomed = true;
    tracing::info!(
        machine = %welcome.machine_id,
        unacknowledged = store.outbox_len()?,
        "connected to {}",
        options.server_url
    );

    // Everything still in the outbox is unacknowledged: send it all again, oldest first.
    let mut sent = 0;
    outbox.notify_one();
    loop {
        tokio::select! {
            message = inbound.message() => {
                let Some(message) = message? else { return Ok(()) };
                match message.message {
                    Some(attach_response::Message::Command(command)) => {
                        commands.send(command).await.context("daemon core stopped")?;
                    }
                    Some(attach_response::Message::EventsAck(ack)) => {
                        store.ack(&ack.session_id, ack.up_to_seq)?;
                    }
                    Some(attach_response::Message::Welcome(_)) | None => {}
                }
            }
            () = outbox.notified() => {
                let rows = store.outbox_after(sent, OUTBOX_BATCH)?;
                if rows.len() == OUTBOX_BATCH {
                    outbox.notify_one();
                }
                for row in rows {
                    send(&tx, attach_request::Message::Event(row.event)).await?;
                    sent = row.id;
                }
            }
            Some(result) = results.recv() => {
                send(&tx, attach_request::Message::CommandResult(result)).await?;
            }
        }
    }
}

async fn send(
    tx: &mpsc::Sender<AttachRequest>,
    message: attach_request::Message,
) -> anyhow::Result<()> {
    tx.send(AttachRequest {
        message: Some(message),
    })
    .await
    .context("server stream closed")
}

/// 50–100% of `delay`, so many daemons don't reconnect in lockstep.
fn jitter(delay: Duration) -> Duration {
    let mut byte = [0u8; 1];
    let _ = getrandom::fill(&mut byte);
    delay.mul_f64(0.5 + f64::from(byte[0]) / 510.0)
}
