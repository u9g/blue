//! The daemon driving the compiled bridge in replay mode: the whole session path (process
//! spawn, framing, Start/Prompt/Interrupt/Close, translation, turn tracking) without Claude.
//! Ignored by default because it needs the compiled bridge; `just test-replay` builds it and
//! runs this.

use std::path::{Path, PathBuf};
use std::time::Duration;

use blue_daemon::session::{Session, SessionConfig};
use blue_protocol::events::v1::{SessionState, session_event::Kind};
use tokio::time::timeout;

fn bridge_path() -> PathBuf {
    std::env::var_os("BLUE_BRIDGE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/bridge")
                .join(format!("claude-bridge{}", std::env::consts::EXE_SUFFIX))
        })
}

async fn next_turn_end(session: &mut Session) -> Kind {
    timeout(Duration::from_secs(30), async {
        loop {
            let event = session.next().await.expect("session ended early");
            match event.event.and_then(|e| e.kind) {
                Some(
                    kind
                    @ (Kind::TurnCompleted(_) | Kind::TurnFailed(_) | Kind::TurnInterrupted(_)),
                ) => return kind,
                _ => continue,
            }
        }
    })
    .await
    .expect("timed out")
}

#[tokio::test]
#[ignore = "needs the compiled bridge; use `just test-replay`"]
async fn replays_a_recorded_session() {
    let recording = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/turns.jsonl");
    let mut session = Session::start(SessionConfig {
        session_id: "replay".into(),
        bridge_path: bridge_path(),
        // Never run in replay mode, but Start requires them.
        claude_path: "claude".into(),
        cwd: std::env::temp_dir(),
        resume_claude_session_id: None,
        model: None,
        replay: Some(recording),
    })
    .await
    .unwrap();

    for (turn, interrupt) in [("t1", false), ("t2", false), ("t3", true), ("t4", false)] {
        session
            .prompt(turn.into(), "replayed".into())
            .await
            .unwrap();
        if interrupt {
            session.interrupt().await.unwrap();
        }
        let end = next_turn_end(&mut session).await;
        match (interrupt, &end) {
            (false, Kind::TurnCompleted(done)) => assert_eq!(done.turn_id, turn),
            (true, Kind::TurnInterrupted(stopped)) => assert_eq!(stopped.turn_id, turn),
            _ => panic!("turn {turn}: unexpected {end:?}"),
        }
    }
    assert!(session.cwd().ends_with("calc"), "{}", session.cwd());
    assert!(session.claude_session_id().is_some());

    session.close().await.unwrap();
    let mut last_state = None;
    while let Some(event) = session.next().await {
        if let Some(Kind::SessionStateChanged(changed)) = event.event.and_then(|e| e.kind) {
            last_state = Some(changed.state());
        }
    }
    assert_eq!(last_state, Some(SessionState::Closed));
}
