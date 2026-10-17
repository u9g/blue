//! How events and sessions look in the terminal.

use blue_protocol::client::v1::{Session, TurnState};
use blue_protocol::events::v1::{SessionState, session_event::Kind};

/// One line per event worth showing; `None` for the rest.
pub fn event_line(kind: &Kind) -> Option<String> {
    match kind {
        Kind::SessionStarted(started) => Some(format!(
            "● Claude started in {} ({})",
            started.cwd, started.model
        )),
        Kind::TurnStarted(_) => None,
        Kind::AssistantText(text) => Some(text.text.clone()),
        Kind::ToolStarted(tool) => Some(format!("  → {}", tool.summary)),
        Kind::ToolFinished(tool) if tool.is_error => Some("  ✗ tool failed".into()),
        Kind::ToolFinished(_) => None,
        Kind::TurnCompleted(done) => Some(format!(
            "✓ done in {:.1}s",
            done.duration_ms as f64 / 1000.0
        )),
        Kind::TurnFailed(failed) => Some(format!("✗ turn failed: {}", failed.reason)),
        Kind::SessionStateChanged(changed) => match changed.state() {
            SessionState::Closed => Some("● session closed".into()),
            SessionState::Failed => Some("● session failed".into()),
            _ => None,
        },
        Kind::RawSdkMessage(_) => None,
    }
}

pub fn session_line(session: &Session) -> String {
    let state = match session.state() {
        SessionState::Starting => "starting",
        SessionState::Idle => "idle",
        SessionState::Running => "running",
        SessionState::Closed => "closed",
        SessionState::Failed => "failed",
        SessionState::Unspecified => "?",
    };
    let turn = session
        .current_turn
        .as_ref()
        .map(|turn| {
            let turn_state = match turn.state() {
                TurnState::Queued => "queued",
                TurnState::Running => "running",
                TurnState::Completed => "completed",
                TurnState::Failed => "failed",
                TurnState::Unspecified => "?",
            };
            format!("turn {} {turn_state}", turn.index + 1)
        })
        .unwrap_or_default();
    format!(
        "{}  {:<12} {:<9} {:<20} {}",
        session.id, session.machine_name, state, turn, session.title
    )
}
