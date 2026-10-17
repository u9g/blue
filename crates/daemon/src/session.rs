//! One Claude session on this machine: a bridge process plus the bookkeeping the server needs —
//! which turn is running, per-session event sequence numbers, and turning a dead bridge into a
//! failed turn.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, bail};
use blue_protocol::bridge::v1::{Close, Prompt, Start, bridge_to_daemon, daemon_to_bridge};
use blue_protocol::daemon::v1::DaemonEvent;
use blue_protocol::events::v1::{
    SessionEvent, SessionState, SessionStateChanged, TurnFailed, TurnStarted, session_event::Kind,
};

use crate::bridge_proc::BridgeProcess;
use crate::recorder::Recorder;
use crate::translate::Translator;

pub struct SessionConfig {
    pub session_id: String,
    pub bridge_path: PathBuf,
    pub claude_path: PathBuf,
    /// Where scouting starts for a new session, or the session's directory when resuming.
    pub cwd: PathBuf,
    pub resume_claude_session_id: Option<String>,
    pub model: Option<String>,
    /// Play back this recording instead of running Claude (see the bridge's replay mode).
    pub replay: Option<PathBuf>,
}

pub struct Session {
    id: String,
    bridge: BridgeProcess,
    translator: Translator,
    seq: u64,
    /// The turn in progress; one at a time.
    turn: Option<String>,
    pending: VecDeque<DaemonEvent>,
    recorder: Option<Recorder>,
    closing: bool,
    exit_error: Option<String>,
    finished: bool,
}

impl Session {
    /// Starts the bridge and sends `Start`. Claude itself starts with the first prompt.
    pub async fn start(config: SessionConfig) -> anyhow::Result<Self> {
        let (mut bridge, _ready) =
            BridgeProcess::spawn_with(&config.bridge_path, config.replay.as_deref()).await?;
        let start = Start {
            session_id: config.session_id.clone(),
            claude_path: path_string(config.claude_path)?,
            resume_claude_session_id: config.resume_claude_session_id,
            model: config.model,
            cwd: path_string(config.cwd)?,
        };
        bridge.send(daemon_to_bridge::Command::Start(start)).await?;

        let mut session = Self {
            id: config.session_id,
            bridge,
            translator: Translator::default(),
            seq: 0,
            turn: None,
            pending: VecDeque::new(),
            recorder: None,
            closing: false,
            exit_error: None,
            finished: false,
        };
        session.state(SessionState::Starting);
        Ok(session)
    }

    /// Writes every raw SDK message to `recorder` (scrubbed), e.g. to capture test fixtures.
    pub fn record_to(&mut self, recorder: Recorder) {
        self.recorder = Some(recorder);
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn claude_session_id(&self) -> Option<&str> {
        self.translator.claude_session_id()
    }

    /// The directory Claude is working in, once the session has started.
    pub fn cwd(&self) -> &str {
        self.translator.cwd()
    }

    pub fn turn_running(&self) -> bool {
        self.turn.is_some()
    }

    pub fn bridge_pid(&self) -> Option<u32> {
        self.bridge.pid()
    }

    /// Starts a turn. Only one turn runs at a time.
    pub async fn prompt(&mut self, turn_id: String, text: String) -> anyhow::Result<()> {
        if let Some(turn) = &self.turn {
            bail!("turn {turn} is still running");
        }
        self.bridge
            .send(daemon_to_bridge::Command::Prompt(Prompt { text }))
            .await?;
        self.emit(
            &turn_id,
            Kind::TurnStarted(TurnStarted {
                turn_id: turn_id.clone(),
            }),
        );
        self.state(SessionState::Running);
        self.turn = Some(turn_id);
        Ok(())
    }

    /// Asks the bridge to finish. Keep calling `next()` until it returns `None`.
    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.closing = true;
        self.bridge
            .send(daemon_to_bridge::Command::Close(Close {}))
            .await
    }

    /// Kills the bridge and Claude immediately.
    pub fn kill(&mut self) {
        self.bridge.kill_tree();
    }

    /// The next event, or `None` once the bridge has exited and every event was delivered.
    /// Cancel-safe.
    pub async fn next(&mut self) -> Option<DaemonEvent> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            if self.finished {
                return None;
            }
            match self.bridge.next_event().await {
                Some(Ok(bridge_to_daemon::Event::SdkMessage(message))) => {
                    self.on_sdk_message(&message.json)
                }
                Some(Ok(bridge_to_daemon::Event::Error(error))) => {
                    tracing::warn!(session = %self.id, "bridge: {}", error.message)
                }
                Some(Ok(bridge_to_daemon::Event::Exited(exited))) => {
                    if !exited.error.is_empty() {
                        self.exit_error = Some(exited.error);
                    }
                }
                Some(Ok(bridge_to_daemon::Event::Ready(_))) => {}
                Some(Err(err)) => self.on_bridge_gone(Some(err)).await,
                None => self.on_bridge_gone(None).await,
            }
        }
    }

    fn on_sdk_message(&mut self, json: &str) {
        if let Some(recorder) = &mut self.recorder {
            recorder.record(json);
        }
        for event in self.translator.translate(json, self.turn.as_deref()) {
            let Some(kind) = event.kind else { continue };
            let ends_turn = matches!(kind, Kind::TurnCompleted(_) | Kind::TurnFailed(_));
            let turn_id = self.turn.clone().unwrap_or_default();
            self.emit(&turn_id, kind);
            if ends_turn {
                self.turn = None;
                self.state(SessionState::Idle);
            }
        }
    }

    async fn on_bridge_gone(&mut self, read_error: Option<anyhow::Error>) {
        let status = self.bridge.wait().await;
        let clean = self.closing
            && read_error.is_none()
            && self.exit_error.is_none()
            && status.as_ref().is_ok_and(|status| status.success());
        if clean {
            self.state(SessionState::Closed);
        } else {
            let mut reason = match (&self.exit_error, read_error, status) {
                (Some(error), _, _) => error.clone(),
                (None, Some(err), _) => format!("{err:#}"),
                (None, None, Ok(status)) => format!("bridge exited unexpectedly ({status})"),
                (None, None, Err(err)) => format!("bridge exited unexpectedly ({err:#})"),
            };
            let stderr = self.bridge.stderr_tail();
            if !stderr.is_empty() {
                reason = format!("{reason}\n{stderr}");
            }
            if let Some(turn_id) = self.turn.take() {
                self.emit(
                    &turn_id,
                    Kind::TurnFailed(TurnFailed {
                        turn_id: turn_id.clone(),
                        reason,
                    }),
                );
            } else {
                tracing::warn!(session = %self.id, "bridge exited: {reason}");
            }
            self.state(SessionState::Failed);
        }
        self.finished = true;
    }

    fn state(&mut self, state: SessionState) {
        let turn_id = self.turn.clone().unwrap_or_default();
        self.emit(
            &turn_id,
            Kind::SessionStateChanged(SessionStateChanged {
                state: state.into(),
            }),
        );
    }

    fn emit(&mut self, turn_id: &str, kind: Kind) {
        self.seq += 1;
        self.pending.push_back(DaemonEvent {
            session_id: self.id.clone(),
            seq: self.seq,
            turn_id: turn_id.to_owned(),
            occurred_at: Some(SystemTime::now().into()),
            event: Some(SessionEvent { kind: Some(kind) }),
        });
    }
}

fn path_string(path: PathBuf) -> anyhow::Result<String> {
    path.into_os_string()
        .into_string()
        .map_err(|path| anyhow::anyhow!("path is not valid UTF-8: {path:?}"))
        .context("session paths must be UTF-8")
}
