//! Real Claude sessions through the compiled bridge. Ignored by default: they need Claude Code
//! installed and logged in, take a minute or two, and cost a little (they use Haiku).
//!
//!     just test-live                # runs them
//!     just record-fixtures          # runs them and copies the recordings to tests/fixtures/
//!
//! Every run records its sessions (scrubbed) to target/fixtures/. The bridge comes from
//! BLUE_BRIDGE_PATH, else target/bridge/ (built by `just bridge`).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use blue_daemon::claude;
use blue_daemon::recorder::Recorder;
use blue_daemon::session::{Session, SessionConfig};
use blue_protocol::events::v1::{SessionState, session_event::Kind};
use common::{bridge_path, close, completed_turn, until};
use sysinfo::{Pid, ProcessesToUpdate, System};

const MODEL: &str = "haiku";

/// A scratch directory holding two small git repos, so scouting has to choose.
fn scratch() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, file, contents) in [
        ("calc", "calc.py", "def add(a, b):\n    return a + b\n"),
        ("todo", "todo.txt", "buy milk\n"),
    ] {
        let repo = dir.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join(file), contents).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
    }
    dir
}

async fn start(cwd: &Path, resume: Option<String>, fixture: &str) -> Session {
    let claude = claude::resolve(None)
        .await
        .expect("claude must be installed");
    let mut session = Session::start(SessionConfig {
        session_id: "test".into(),
        bridge_path: bridge_path(),
        claude_path: claude.path,
        cwd: cwd.to_owned(),
        resume_claude_session_id: resume,
        model: Some(MODEL.into()),
        replay: None,
    })
    .await
    .expect("start session");
    let recordings = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures");
    std::fs::create_dir_all(&recordings).unwrap();
    session.record_to(Recorder::create(&recordings.join(format!("{fixture}.jsonl"))).unwrap());
    session
}

#[tokio::test]
#[ignore = "runs real Claude sessions; use `just test-live`"]
async fn turns_and_resume() {
    let scratch = scratch();
    let calc = scratch.path().join("calc");
    let mut session = start(scratch.path(), None, "turns").await;

    // Turn 1: scouting picks the calc repo, and Claude answers from it.
    let answer = completed_turn(
        &mut session,
        "t1",
        "In the calc project, what function does calc.py define? Answer with just its name.",
    )
    .await;
    assert!(answer.contains("add"), "{answer}");
    assert_eq!(
        Path::new(session.cwd()).canonicalize().unwrap(),
        calc.canonicalize().unwrap()
    );

    // Turn 2: a follow-up in the same session edits the file.
    completed_turn(
        &mut session,
        "t2",
        "Add a subtract(a, b) function to calc.py. Don't commit.",
    )
    .await;
    let source = std::fs::read_to_string(calc.join("calc.py")).unwrap();
    assert!(source.contains("def subtract"), "{source}");

    // Turn 3: one more turn in the same session.
    completed_turn(&mut session, "t3", "Reply with just the word ok.").await;

    let claude_session_id = session.claude_session_id().unwrap().to_owned();
    let cwd = PathBuf::from(session.cwd());
    close(session).await;

    // Resume: a new bridge picks the conversation back up in the same directory.
    let mut resumed = start(&cwd, Some(claude_session_id.clone()), "resume").await;
    let answer = completed_turn(
        &mut resumed,
        "t4",
        "What function did you add to calc.py earlier? Answer with just its name.",
    )
    .await;
    assert!(answer.contains("subtract"), "{answer}");
    assert_eq!(
        resumed.claude_session_id(),
        Some(claude_session_id.as_str())
    );
    close(resumed).await;
}

#[tokio::test]
#[ignore = "runs a real Claude session; use `just test-live`"]
async fn killing_the_bridge_kills_claude() {
    let scratch = scratch();
    let mut session = start(&scratch.path().join("todo"), None, "killed").await;
    session
        .prompt(
            "t1".into(),
            "In the todo project, run exactly this shell command with Bash, in the foreground: `echo started; sleep 120; echo finished`"
                .into(),
        )
        .await
        .unwrap();
    until(
        &mut session,
        |kind| matches!(kind, Kind::ToolStarted(t) if t.tool == "Bash"),
    )
    .await;
    // Let the shell command actually start.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let bridge = Pid::from_u32(session.bridge_pid().unwrap());
    let tree = descendants(bridge);
    assert!(
        tree.len() >= 2,
        "expected claude and the shell running under the bridge, got {tree:?}"
    );

    session.kill();
    let events = until(
        &mut session,
        |kind| matches!(kind, Kind::SessionStateChanged(s) if s.state() == SessionState::Failed),
    )
    .await;
    assert!(
        events
            .iter()
            .any(|kind| matches!(kind, Kind::TurnFailed(f) if f.turn_id == "t1")),
        "{events:#?}"
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    // A PID only counts as the same process if its start time matches too: Windows reuses
    // PIDs, and short-lived processes (Claude Code runs `git` at startup) exit on their own.
    let alive: Vec<_> = tree
        .iter()
        .filter_map(|&(pid, started)| system.process(pid).filter(|p| p.start_time() == started))
        .map(|process| {
            format!(
                "{} {:?} parent={:?} started={} cmd={:?}",
                process.pid(),
                process.name(),
                process.parent(),
                process.start_time(),
                process.cmd()
            )
        })
        .collect();
    assert!(alive.is_empty(), "still running after kill: {alive:?}");
}

/// Every process started (directly or not) by `root`, with its start time.
fn descendants(root: Pid) -> Vec<(Pid, u64)> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut found = vec![(root, system.process(root).unwrap().start_time())];
    let mut i = 0;
    while i < found.len() {
        let (parent, parent_started) = found[i];
        found.extend(
            system
                .processes()
                .values()
                // A child can't predate its parent; older ones only share a reused parent PID.
                .filter(|p| p.parent() == Some(parent) && p.start_time() >= parent_started)
                .map(|p| (p.pid(), p.start_time())),
        );
        i += 1;
    }
    found.split_off(1)
}
