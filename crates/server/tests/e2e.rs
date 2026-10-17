//! Phase 2's exit criteria, end to end: real `blue-server` and `blue-daemon` processes, the
//! compiled bridge replaying a recorded session (slowed down so turns can be interrupted), and
//! a client driving it over gRPC. Kills the server mid-turn, then the daemon mid-turn, and
//! checks that no event went missing.
//!
//! Ignored by default: needs the compiled bridge and blue-daemon (`just test-e2e` builds both).

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use blue_common::Bearer;
use blue_protocol::client::v1::client_service_client::ClientServiceClient;
use blue_protocol::client::v1::{
    CreateSessionRequest, Event, SendPromptRequest, SubscribeRequest, event,
};
use blue_protocol::events::v1::session_event::Kind;
use common::{Postgres, TestServer, session_kind};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

const DEADLINE: Duration = Duration::from_secs(60);

#[tokio::test]
#[ignore = "needs the compiled bridge and blue-daemon; use `just test-e2e`"]
async fn tasks_survive_server_and_daemon_restarts() {
    let postgres = Postgres::start().await;
    let database_url = postgres.new_database().await;
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}");
    let mut server = start_server(port, &database_url).await;

    // A user, and a daemon paired through the real CLI flow.
    let harness = TestServer::start(common::config(&database_url)).await;
    let token = harness.user_token().await;
    let code = harness
        .auth(Some(&token))
        .await
        .create_pairing_code(blue_protocol::auth::v1::CreatePairingCodeRequest {})
        .await
        .unwrap()
        .into_inner()
        .code;
    let db = harness.db.clone();
    harness.stop().await;
    let daemon_dir = tempfile::tempdir().unwrap();
    let paired = daemon_command(daemon_dir.path())
        .args([
            "pair", "--server", &url, "--code", &code, "--name", "desktop",
        ])
        .status()
        .await
        .unwrap();
    assert!(paired.success());
    let mut daemon = start_daemon(daemon_dir.path());

    let mut client = ClientServiceClient::with_interceptor(
        blue_common::connect(&url).await.unwrap(),
        Bearer::new(&token).unwrap(),
    );
    let created = client
        .create_session(CreateSessionRequest {
            machine: "desktop".into(),
            instructions: "in the calc repo, list the functions".into(),
            title: "Calc".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let session_id = created.session.unwrap().id;
    let mut cursor = 0;

    // 1. A full turn.
    let events = wait_for(&url, &token, &mut cursor, turn_ended(&created.turn_id)).await;
    assert!(matches!(
        session_kind(events.last().unwrap()),
        Some(Kind::TurnCompleted(_))
    ));

    // 2. Kill the server mid-turn; the daemon keeps working, buffers, and replays.
    let turn = send(&mut client, &session_id, "add a subtract function").await;
    wait_for(&url, &token, &mut cursor, tool_started(&turn)).await;
    server.kill().await.unwrap();
    sleep(Duration::from_millis(500)).await;
    server = start_server(port, &database_url).await;
    let mut client = ClientServiceClient::with_interceptor(
        blue_common::connect(&url).await.unwrap(),
        Bearer::new(&token).unwrap(),
    );
    let events = wait_for(&url, &token, &mut cursor, turn_ended(&turn)).await;
    assert!(
        matches!(
            session_kind(events.last().unwrap()),
            Some(Kind::TurnCompleted(_))
        ),
        "{events:#?}"
    );
    assert_no_gaps(&db, &session_id).await;

    // 3. Kill the daemon mid-turn: that turn fails, and the session resumes on the next one.
    let turn = send(&mut client, &session_id, "now add tests").await;
    wait_for(&url, &token, &mut cursor, turn_started(&turn)).await;
    daemon.kill().await.unwrap();
    let mut daemon = start_daemon(daemon_dir.path());
    let events = wait_for(&url, &token, &mut cursor, turn_ended(&turn)).await;
    match session_kind(events.last().unwrap()) {
        Some(Kind::TurnFailed(failed)) => {
            assert!(failed.reason.contains("daemon stopped"), "{failed:?}")
        }
        other => panic!("expected the interrupted turn to fail, got {other:?}"),
    }
    let turn = send(&mut client, &session_id, "try again").await;
    let events = wait_for(&url, &token, &mut cursor, turn_ended(&turn)).await;
    assert!(
        matches!(
            session_kind(events.last().unwrap()),
            Some(Kind::TurnCompleted(_))
        ),
        "{:?}",
        session_kind(events.last().unwrap())
    );
    assert_no_gaps(&db, &session_id).await;

    daemon.kill().await.unwrap();
    server.kill().await.unwrap();
}

/// The daemon numbers each session's events 1, 2, 3, …; all of them must have arrived.
async fn assert_no_gaps(db: &sqlx::PgPool, session_id: &str) {
    let seqs: Vec<i64> = sqlx::query_scalar(
        "SELECT seq FROM events WHERE session_id = $1::uuid AND seq IS NOT NULL ORDER BY seq",
    )
    .bind(session_id)
    .fetch_all(db)
    .await
    .unwrap();
    let expected: Vec<i64> = (1..=seqs.len() as i64).collect();
    assert_eq!(seqs, expected, "events missing");
}

async fn send(client: &mut common::Client, session_id: &str, text: &str) -> String {
    client
        .send_prompt(SendPromptRequest {
            session_id: session_id.into(),
            instructions: text.into(),
        })
        .await
        .unwrap()
        .into_inner()
        .turn_id
}

fn turn_of(event: &Event) -> Option<&str> {
    match event.kind.as_ref()? {
        event::Kind::Session(envelope) => Some(&envelope.turn_id),
        _ => None,
    }
}

fn turn_ended(turn: &str) -> impl Fn(&Event) -> bool + '_ {
    move |event| turn_of(event) == Some(turn) && session_kind(event).is_some_and(Kind::ends_turn)
}

fn turn_started(turn: &str) -> impl Fn(&Event) -> bool + '_ {
    move |event| matches!(session_kind(event), Some(Kind::TurnStarted(started)) if started.turn_id == turn)
}

fn tool_started(turn: &str) -> impl Fn(&Event) -> bool + '_ {
    move |event| {
        turn_of(event) == Some(turn) && matches!(session_kind(event), Some(Kind::ToolStarted(_)))
    }
}

/// Follows events from `cursor` until one matches, resubscribing across server restarts.
async fn wait_for(
    url: &str,
    token: &str,
    cursor: &mut u64,
    done: impl Fn(&Event) -> bool,
) -> Vec<Event> {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let Ok(channel) = blue_common::connect(url).await else {
            sleep(Duration::from_millis(200)).await;
            continue;
        };
        let mut client =
            ClientServiceClient::with_interceptor(channel, Bearer::new(token).unwrap());
        let Ok(stream) = client
            .subscribe(SubscribeRequest {
                cursor: Some(*cursor),
                session_ids: Vec::new(),
                include_raw: false,
            })
            .await
        else {
            sleep(Duration::from_millis(200)).await;
            continue;
        };
        let mut stream = stream.into_inner();
        while let Ok(Ok(Some(response))) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            stream.message(),
        )
        .await
        {
            let event = response.event.unwrap();
            *cursor = event.cursor;
            let stop = done(&event);
            seen.push(event);
            if stop {
                return seen;
            }
        }
    }
    let summary: Vec<String> = seen
        .iter()
        .map(|event| {
            format!(
                "{} {:?} {:?}",
                event.cursor,
                turn_of(event),
                session_kind(event)
            )
        })
        .collect();
    panic!(
        "timed out at cursor {cursor}; saw:
{}",
        summary.join(
            "
"
        )
    );
}

async fn start_server(port: u16, database_url: &str) -> Child {
    let child = Command::new(env!("CARGO_BIN_EXE_blue-server"))
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .env("BLUE_DATABASE_URL", database_url)
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Ok(channel) = blue_common::connect(&url).await {
            let check = HealthClient::new(channel)
                .check(HealthCheckRequest {
                    service: String::new(),
                })
                .await;
            if check.is_ok_and(|response| response.into_inner().status == 1) {
                return child;
            }
        }
        assert!(Instant::now() < deadline, "server didn't become ready");
        sleep(Duration::from_millis(100)).await;
    }
}

fn start_daemon(dir: &Path) -> Child {
    let recording = dir.join("recording.jsonl");
    if !recording.exists() {
        // Recordings are scrubbed to `<HOME>`; resuming needs the session's real, absolute
        // directory back.
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../daemon/tests/fixtures/turns.jsonl");
        let home = serde_json::to_string(&std::env::home_dir().unwrap()).unwrap();
        let text = std::fs::read_to_string(fixture)
            .unwrap()
            .replace("<HOME>", home.trim_matches('"'));
        std::fs::write(&recording, text).unwrap();
    }
    let mut command = daemon_command(dir);
    command
        .args(["start", "--bridge"])
        .arg(bridge_path())
        .arg("--replay")
        .arg(recording)
        .env("BLUE_BRIDGE_REPLAY_DELAY_MS", "40")
        .env("RUST_LOG", "warn,blue_daemon=info");
    command.spawn().unwrap()
}

fn daemon_command(dir: &Path) -> Command {
    let mut command = Command::new(sibling_binary("blue-daemon"));
    command
        .arg("--dir")
        .arg(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

/// Binaries from other packages, built into the same target directory as this test.
fn sibling_binary(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_blue-server"))
        .with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} not found; build it first (cargo build -p blue-daemon)",
        path.display()
    );
    path
}

/// BLUE_BRIDGE_PATH, else target/bridge/ (built by `just bridge`).
fn bridge_path() -> PathBuf {
    std::env::var_os("BLUE_BRIDGE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/bridge")
                .join(format!("claude-bridge{}", std::env::consts::EXE_SUFFIX))
        })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
