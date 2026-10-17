//! The daemon driving the compiled bridge in replay mode: the whole session path (process
//! spawn, framing, Start/Prompt/Close, translation, turn tracking) without Claude.
//! Ignored by default because it needs the compiled bridge; `just test-replay` builds it and
//! runs this.

mod common;

use std::path::Path;

use blue_daemon::session::{Session, SessionConfig};
use common::{bridge_path, close, completed_turn};

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
        forward_raw: false,
    })
    .await
    .unwrap();

    for turn in ["t1", "t2", "t3"] {
        completed_turn(&mut session, turn, "replayed").await;
    }
    assert!(session.cwd().ends_with("calc"), "{}", session.cwd());
    assert!(session.claude_session_id().is_some());
    close(session).await;
}
