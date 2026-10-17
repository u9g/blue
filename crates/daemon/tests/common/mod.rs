//! Helpers shared by the tests that drive the compiled bridge (live.rs, replay.rs).

use std::path::{Path, PathBuf};
use std::time::Duration;

use blue_daemon::session::Session;
use blue_protocol::events::v1::{SessionState, session_event::Kind};
use tokio::time::timeout;

const TURN_TIMEOUT: Duration = Duration::from_secs(180);

/// BLUE_BRIDGE_PATH, else target/bridge/ (built by `just bridge`).
pub fn bridge_path() -> PathBuf {
    std::env::var_os("BLUE_BRIDGE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/bridge")
                .join(format!("claude-bridge{}", std::env::consts::EXE_SUFFIX))
        })
}

/// Collects events until one matches `done`, which is returned last.
pub async fn until(session: &mut Session, done: impl Fn(&Kind) -> bool) -> Vec<Kind> {
    let mut seen = Vec::new();
    timeout(TURN_TIMEOUT, async {
        while let Some(event) = session.next().await {
            let Some(kind) = event.into_kind() else {
                continue;
            };
            let stop = done(&kind);
            seen.push(kind);
            if stop {
                return;
            }
        }
        panic!("session ended early: {seen:#?}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"));
    seen
}

/// Runs one turn, asserts it completed, and returns its result text.
pub async fn completed_turn(session: &mut Session, turn: &str, text: &str) -> String {
    session.prompt(turn.into(), text.into()).await.unwrap();
    let events = until(session, Kind::ends_turn).await;
    match events.last() {
        Some(Kind::TurnCompleted(done)) => {
            assert_eq!(done.turn_id, turn);
            done.result_text.clone()
        }
        other => panic!("turn {turn} did not complete: {other:#?}\n{events:#?}"),
    }
}

/// Closes the session and asserts it ends `Closed`.
pub async fn close(mut session: Session) {
    session.close().await.unwrap();
    let events = until(
        &mut session,
        |kind| matches!(kind, Kind::SessionStateChanged(s) if s.state() != SessionState::Idle),
    )
    .await;
    assert!(
        matches!(events.last(), Some(Kind::SessionStateChanged(s)) if s.state() == SessionState::Closed),
        "{events:#?}"
    );
}
