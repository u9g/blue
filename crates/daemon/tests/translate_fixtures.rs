//! Snapshot tests: recorded SDK message streams (tests/fixtures/*.jsonl, captured from real
//! Claude by `just record-fixtures`) translated into Blue session events.
//!
//! Recordings mark where the daemon interrupted a turn (`recorder::INTERRUPT_MARKER`), which is
//! what tells the translator that the next `result` means "interrupted".

use std::fmt::Write;
use std::path::Path;

use blue_daemon::recorder::INTERRUPT_MARKER;
use blue_daemon::translate::{Translator, Turn};
use blue_protocol::events::v1::session_event::Kind;

fn translate_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jsonl"));
    let recording = std::fs::read_to_string(&path).unwrap();

    let mut translator = Translator::default();
    let mut out = String::new();
    let mut turn = 1;
    let mut interrupted = false;
    for line in recording.lines().filter(|line| !line.trim().is_empty()) {
        if line == INTERRUPT_MARKER {
            interrupted = true;
            continue;
        }
        let turn_id = format!("t{turn}");
        let events = translator.translate(
            line,
            Some(Turn {
                id: &turn_id,
                interrupted,
            }),
        );
        for kind in events.into_iter().filter_map(|event| event.kind) {
            let ends_turn = matches!(
                kind,
                Kind::TurnCompleted(_) | Kind::TurnFailed(_) | Kind::TurnInterrupted(_)
            );
            writeln!(out, "{}", describe(kind)).unwrap();
            if ends_turn {
                turn += 1;
                interrupted = false;
            }
        }
    }
    out
}

fn describe(kind: Kind) -> String {
    match kind {
        Kind::SessionStarted(e) => format!("SessionStarted model={} cwd={}", e.model, e.cwd),
        Kind::AssistantText(e) => format!("AssistantText {:?}", e.text),
        Kind::ToolStarted(e) => format!("ToolStarted {} {:?}", e.tool, e.summary),
        Kind::ToolFinished(e) => format!("ToolFinished is_error={}", e.is_error),
        Kind::TurnCompleted(e) => format!(
            "TurnCompleted {} num_turns={} result={:?}",
            e.turn_id, e.num_turns, e.result_text
        ),
        Kind::TurnFailed(e) => format!("TurnFailed {} {:?}", e.turn_id, e.reason),
        Kind::TurnInterrupted(e) => format!("TurnInterrupted {}", e.turn_id),
        other => format!("{other:?}"),
    }
}

#[test]
fn turns() {
    insta::assert_snapshot!(translate_fixture("turns"));
}

#[test]
fn resume() {
    insta::assert_snapshot!(translate_fixture("resume"));
}

#[test]
fn killed() {
    insta::assert_snapshot!(translate_fixture("killed"));
}
