//! The running daemon: carries out server commands on local sessions, and turns session events
//! into outbox entries for the connection to deliver.
//!
//! One task (this "core") owns the session table and the store's write path; each live session
//! runs in its own task and talks to the core over channels. A session's bridge is started on
//! its first prompt, and started again (resuming the Claude conversation) on the first prompt
//! after it exited.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, bail};
use blue_protocol::daemon::v1::{
    CloseSession, Command, CommandResult, DaemonEvent, SendPrompt, command,
};
use blue_protocol::events::v1::{
    SessionEvent, SessionState, SessionStateChanged, TurnFailed, session_event::Kind,
};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::connection::{self, ConnectionOptions};
use crate::session::{Session, SessionConfig};
use crate::store::{Store, StoredSession};

/// Everything needed to start bridges on this machine.
#[derive(Clone)]
pub struct Launcher {
    pub bridge_path: PathBuf,
    pub claude_path: PathBuf,
    /// Where scouting starts for new sessions.
    pub start_directory: PathBuf,
    pub forward_raw: bool,
    /// Play back this recording instead of running Claude (for testing).
    pub replay: Option<PathBuf>,
}

/// Runs the daemon until `shutdown` resolves, or until the server refuses it (revoked token).
pub async fn run(
    connection: ConnectionOptions,
    launcher: Launcher,
    store: Arc<Store>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    recover(&store)?;
    let outbox = Arc::new(Notify::new());
    let (commands_tx, commands_rx) = mpsc::channel(64);
    let (results_tx, results_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let core = Core {
        store: store.clone(),
        outbox: outbox.clone(),
        launcher,
        results: results_tx,
        sessions: HashMap::new(),
        events_tx,
        events_rx,
    };
    let mut connection = tokio::spawn(connection::run(
        connection,
        store,
        outbox,
        commands_tx,
        results_rx,
    ));
    tokio::select! {
        result = core.run(commands_rx) => result,
        result = &mut connection => result.context("connection task panicked")?,
        () = shutdown => {
            connection.abort();
            Ok(())
        }
    }
}

/// Turns interrupted by a daemon crash or restart can't continue: their bridges died with the
/// old process. Fail them, so the server moves on; the next prompt resumes the conversation.
fn recover(store: &Store) -> anyhow::Result<()> {
    for session in store.sessions()? {
        let Some(turn_id) = session.current_turn.clone() else {
            continue;
        };
        tracing::warn!(session = %session.id, turn = %turn_id, "failing a turn interrupted by a daemon restart");
        let reason = "the daemon stopped while this turn was running".to_owned();
        store.append(event(
            &session.id,
            &turn_id,
            Kind::TurnFailed(TurnFailed {
                turn_id: turn_id.clone(),
                reason,
            }),
        ))?;
        store.append(event(&session.id, "", state(SessionState::Failed)))?;
    }
    Ok(())
}

enum SessionCmd {
    Prompt {
        turn_id: String,
        text: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Close,
}

enum SessionOut {
    Event(DaemonEvent),
    Ended(String),
}

struct Core {
    store: Arc<Store>,
    outbox: Arc<Notify>,
    launcher: Launcher,
    results: mpsc::UnboundedSender<CommandResult>,
    /// Sessions with a live bridge.
    sessions: HashMap<String, mpsc::UnboundedSender<SessionCmd>>,
    events_tx: mpsc::UnboundedSender<SessionOut>,
    events_rx: mpsc::UnboundedReceiver<SessionOut>,
}

impl Core {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(out) = self.events_rx.recv() => self.on_session_out(out)?,
                command = commands.recv() => {
                    let Some(command) = command else { return Ok(()) };
                    let command_id = command.command_id.clone();
                    let result = self.handle(command).await;
                    if let Err(err) = &result {
                        tracing::warn!(command = %command_id, "command failed: {err:#}");
                    }
                    let _ = self.results.send(CommandResult {
                        command_id,
                        ok: result.is_ok(),
                        error_code: String::new(),
                        error_message: result.err().map(|err| format!("{err:#}")).unwrap_or_default(),
                    });
                }
            }
        }
    }

    async fn handle(&mut self, command: Command) -> anyhow::Result<()> {
        let id = command.command_id;
        if self.store.command_seen(&id)? {
            return Ok(());
        }
        match command.command.context("empty command")? {
            command::Command::StartSession(start) => {
                self.store
                    .insert_session(&start.session_id, start.model.as_deref())?;
                self.store.record_command(&id)?;
            }
            command::Command::SendPrompt(prompt) => self.prompt(&id, prompt).await?,
            command::Command::CloseSession(CloseSession { session_id }) => {
                match self.sessions.get(&session_id) {
                    Some(session) if session.send(SessionCmd::Close).is_ok() => {}
                    // No bridge running: nothing to wind down.
                    _ => {
                        if self.store.session(&session_id)?.is_some() {
                            self.emit(event(&session_id, "", state(SessionState::Closed)))?;
                        }
                    }
                }
                self.store.record_command(&id)?;
            }
        }
        Ok(())
    }

    async fn prompt(&mut self, command_id: &str, prompt: SendPrompt) -> anyhow::Result<()> {
        let stored = self
            .store
            .session(&prompt.session_id)?
            .with_context(|| format!("unknown session {}", prompt.session_id))?;
        if stored.state == SessionState::Closed {
            bail!("session {} is closed", stored.id);
        }
        self.store
            .begin_turn(&stored.id, &prompt.turn_id, command_id)?;
        let result = self.send_prompt(&stored, &prompt).await;
        if result.is_err() {
            self.store.abort_turn(&stored.id, command_id)?;
        }
        result
    }

    async fn send_prompt(
        &mut self,
        stored: &StoredSession,
        prompt: &SendPrompt,
    ) -> anyhow::Result<()> {
        // A bridge that just exited may still have events in flight; take them in first so
        // they keep their order ahead of the new bridge's.
        for _ in 0..2 {
            let session = self.live_session(stored).await?;
            let (reply, replied) = oneshot::channel();
            let sent = session.send(SessionCmd::Prompt {
                turn_id: prompt.turn_id.clone(),
                text: prompt.text.clone(),
                reply,
            });
            if sent.is_ok()
                && let Ok(result) = replied.await
            {
                return result;
            }
            self.drain_until_ended(&stored.id).await?;
        }
        bail!("the session's bridge keeps exiting")
    }

    /// The session's live bridge, starting one if needed.
    async fn live_session(
        &mut self,
        stored: &StoredSession,
    ) -> anyhow::Result<mpsc::UnboundedSender<SessionCmd>> {
        if let Some(session) = self.sessions.get(&stored.id) {
            if !session.is_closed() {
                return Ok(session.clone());
            }
            self.drain_until_ended(&stored.id).await?;
        }
        // Resuming needs the directory the conversation ran in; a session that never got that
        // far starts over with scouting.
        let (cwd, resume) = match (&stored.cwd, &stored.claude_session_id) {
            (Some(cwd), Some(id)) if !cwd.is_empty() => (PathBuf::from(cwd), Some(id.clone())),
            _ => (self.launcher.start_directory.clone(), None),
        };
        let session = Session::start(SessionConfig {
            session_id: stored.id.clone(),
            bridge_path: self.launcher.bridge_path.clone(),
            claude_path: self.launcher.claude_path.clone(),
            cwd,
            resume_claude_session_id: resume,
            model: stored.model.clone(),
            replay: self.launcher.replay.clone(),
            forward_raw: self.launcher.forward_raw,
        })
        .await
        .context("start the bridge")?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(drive(session, rx, self.events_tx.clone()));
        self.sessions.insert(stored.id.clone(), tx.clone());
        Ok(tx)
    }

    async fn drain_until_ended(&mut self, session_id: &str) -> anyhow::Result<()> {
        if !self.sessions.contains_key(session_id) {
            return Ok(());
        }
        while let Some(out) = self.events_rx.recv().await {
            let ended = matches!(&out, SessionOut::Ended(id) if id == session_id);
            self.on_session_out(out)?;
            if ended {
                break;
            }
        }
        Ok(())
    }

    fn on_session_out(&mut self, out: SessionOut) -> anyhow::Result<()> {
        match out {
            SessionOut::Event(event) => self.emit(event),
            SessionOut::Ended(session_id) => {
                self.sessions.remove(&session_id);
                Ok(())
            }
        }
    }

    fn emit(&self, event: DaemonEvent) -> anyhow::Result<()> {
        self.store.append(event)?;
        self.outbox.notify_one();
        Ok(())
    }
}

/// Owns one live session: forwards commands to it and its events to the core.
async fn drive(
    mut session: Session,
    mut commands: mpsc::UnboundedReceiver<SessionCmd>,
    out: mpsc::UnboundedSender<SessionOut>,
) {
    let id = session.id().to_owned();
    let mut commands_open = true;
    loop {
        tokio::select! {
            command = commands.recv(), if commands_open => match command {
                Some(SessionCmd::Prompt { turn_id, text, reply }) => {
                    let _ = reply.send(session.prompt(turn_id, text).await);
                }
                Some(SessionCmd::Close) => {
                    if let Err(err) = session.close().await {
                        tracing::warn!(session = %id, "close: {err:#}");
                    }
                }
                // The daemon is shutting down.
                None => {
                    session.kill();
                    commands_open = false;
                }
            },
            event = session.next() => match event {
                Some(event) => {
                    let _ = out.send(SessionOut::Event(event));
                }
                None => break,
            },
        }
    }
    let _ = out.send(SessionOut::Ended(id));
}

/// An event the daemon makes itself; the store assigns its seq.
fn event(session_id: &str, turn_id: &str, kind: Kind) -> DaemonEvent {
    DaemonEvent {
        session_id: session_id.into(),
        seq: 0,
        turn_id: turn_id.into(),
        occurred_at: Some(SystemTime::now().into()),
        event: Some(SessionEvent { kind: Some(kind) }),
    }
}

fn state(state: SessionState) -> Kind {
    Kind::SessionStateChanged(SessionStateChanged {
        state: state.into(),
    })
}
