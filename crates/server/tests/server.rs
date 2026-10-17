//! The server against real Postgres, with a fake daemon and clients over real gRPC streams:
//! auth, routing, the turn queue, event dedupe, reconnect reconciliation and fan-out.

mod common;

use blue_protocol::auth::v1::{GetCurrentUserRequest, RevokeMachineRequest};
use blue_protocol::client::v1::{
    AmbiguousTarget, CreateSessionRequest, GetSessionRequest, ListMachinesRequest,
    ListSessionsRequest, SendPromptRequest, TurnState, event,
};
use blue_protocol::daemon::v1::{SessionSnapshot, command};
use blue_protocol::events::v1::{
    AssistantText, SessionStarted, SessionState, SessionStateChanged, ToolStarted, TurnCompleted,
    TurnStarted, session_event::Kind,
};
use common::{FakeDaemon, Postgres, TestServer, config, events_until, session_kind};
use prost::Message;
use tonic::Code;

fn completed(turn_id: &str) -> Kind {
    Kind::TurnCompleted(TurnCompleted {
        turn_id: turn_id.into(),
        result_text: "done".into(),
        duration_ms: 1200,
        num_turns: 1,
    })
}

fn started(turn_id: &str) -> Kind {
    Kind::TurnStarted(TurnStarted {
        turn_id: turn_id.into(),
    })
}

fn state(state: SessionState) -> Kind {
    Kind::SessionStateChanged(SessionStateChanged {
        state: state.into(),
    })
}

#[tokio::test]
async fn rejects_missing_and_wrong_credentials() {
    let postgres = Postgres::start().await;
    let server = TestServer::start(config(&postgres.new_database().await)).await;
    let user = server.user_token().await;
    let machine = server.pair(&user, "desktop").await;

    let no_token = server
        .client("")
        .await
        .list_machines(ListMachinesRequest {})
        .await;
    assert_eq!(no_token.unwrap_err().code(), Code::Unauthenticated);
    let wrong = server
        .client("blue_s_nope")
        .await
        .list_machines(ListMachinesRequest {})
        .await;
    assert_eq!(wrong.unwrap_err().code(), Code::Unauthenticated);
    // Machine tokens don't work on the client API, and user tokens don't work for daemons.
    let as_machine = server
        .client(&machine)
        .await
        .list_machines(ListMachinesRequest {})
        .await;
    assert_eq!(as_machine.unwrap_err().code(), Code::Unauthenticated);
    let as_user = FakeDaemon::attach(&server, &user, vec![]).await;
    assert_eq!(as_user.err().unwrap().code(), Code::Unauthenticated);
    // The voice token works on the client API but can't manage auth.
    server
        .client("voice-secret")
        .await
        .list_machines(ListMachinesRequest {})
        .await
        .unwrap();
    let voice_auth = server
        .auth(Some("voice-secret"))
        .await
        .get_current_user(GetCurrentUserRequest {})
        .await;
    assert_eq!(voice_auth.unwrap_err().code(), Code::Unauthenticated);

    // Pairing codes are single-use.
    let code = server
        .auth(Some(&user))
        .await
        .create_pairing_code(blue_protocol::auth::v1::CreatePairingCodeRequest {})
        .await
        .unwrap()
        .into_inner()
        .code;
    let claim = |name: &'static str| {
        let code = code.clone();
        let server = &server;
        async move {
            server
                .auth(None)
                .await
                .claim_pairing_code(blue_protocol::auth::v1::ClaimPairingCodeRequest {
                    code,
                    machine_name: name.into(),
                })
                .await
        }
    };
    // Names are unique regardless of case.
    assert_eq!(
        claim("DESKTOP").await.unwrap_err().code(),
        Code::AlreadyExists
    );
    claim("laptop").await.unwrap();
    assert_eq!(
        claim("other").await.unwrap_err().code(),
        Code::PermissionDenied
    );

    // Revoking a machine drops its stream and its token.
    let daemon = FakeDaemon::attach(&server, &machine, vec![]).await.unwrap();
    server
        .auth(Some(&user))
        .await
        .revoke_machine(RevokeMachineRequest {
            machine: "desktop".into(),
        })
        .await
        .unwrap();
    daemon.closed().await;
    let again = FakeDaemon::attach(&server, &machine, vec![]).await;
    assert_eq!(again.err().unwrap().code(), Code::Unauthenticated);

    server.stop().await;
}

#[tokio::test]
async fn routes_turns_one_at_a_time_and_fans_out_events() {
    let postgres = Postgres::start().await;
    let server = TestServer::start(config(&postgres.new_database().await)).await;
    let user = server.user_token().await;
    let machine = server.pair(&user, "Desktop").await;
    let mut client = server.client(&user).await;
    let mut events = server.subscribe(&user, Some(0)).await;

    // Created while the machine is offline: queued until it connects.
    let created = client
        .create_session(CreateSessionRequest {
            machine: "desk".into(),
            instructions: "in the blue repo, add a health endpoint".into(),
            title: "Health endpoint".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let session = created.session.unwrap();
    let turn0 = created.turn_id;
    assert_eq!(session.machine_name, "Desktop");
    assert_eq!(session.current_turn.unwrap().state(), TurnState::Queued);

    let mut daemon = FakeDaemon::attach(&server, &machine, vec![]).await.unwrap();
    let start = daemon.command().await;
    assert_eq!(start.command_id, format!("start:{}", session.id));
    assert!(matches!(
        start.command,
        Some(command::Command::StartSession(_))
    ));
    let prompt = daemon.command().await;
    match prompt.command {
        Some(command::Command::SendPrompt(prompt)) => {
            assert_eq!(prompt.turn_id, turn0);
            assert_eq!(prompt.text, "in the blue repo, add a health endpoint");
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    daemon.result(&start.command_id, true, "").await;

    // A follow-up while turn 0 runs is queued, not sent.
    let turn1 = client
        .send_prompt(SendPromptRequest {
            session_id: session.id.clone(),
            instructions: "also add a test".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .turn_id;
    daemon.no_command().await;

    let s = &session.id;
    daemon.event(s, 1, "", state(SessionState::Starting)).await;
    daemon.event(s, 2, &turn0, started(&turn0)).await;
    daemon
        .event(s, 3, &turn0, state(SessionState::Running))
        .await;
    daemon
        .event(
            s,
            4,
            &turn0,
            Kind::SessionStarted(SessionStarted {
                claude_session_id: "claude-1".into(),
                model: "haiku".into(),
                cwd: "/home/me/blue".into(),
            }),
        )
        .await;
    daemon
        .event(
            s,
            5,
            &turn0,
            Kind::ToolStarted(ToolStarted {
                tool_use_id: "t".into(),
                tool: "Edit".into(),
                summary: "Edit src/main.rs".into(),
            }),
        )
        .await;
    daemon
        .event(
            s,
            6,
            &turn0,
            Kind::AssistantText(AssistantText {
                text: "Added it.".into(),
            }),
        )
        .await;
    daemon.event(s, 7, &turn0, completed(&turn0)).await;

    // Turn 0 is done, so turn 1 goes out.
    match daemon.command().await.command {
        Some(command::Command::SendPrompt(prompt)) => assert_eq!(prompt.turn_id, turn1),
        other => panic!("expected SendPrompt, got {other:?}"),
    }

    let got = client
        .get_session(GetSessionRequest {
            session_id: s.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(got.events.len(), 7);
    let current = got.session.unwrap().current_turn.unwrap();
    assert_eq!(current.id, turn1);
    assert_eq!(current.state(), TurnState::Queued);

    // Subscribers see the machine come online, then every session event in order.
    let seen = events_until(&mut events, |event| {
        matches!(session_kind(event), Some(Kind::TurnCompleted(_)))
    })
    .await;
    assert!(matches!(
        seen[0].kind,
        Some(event::Kind::MachineStatus(ref status)) if status.online
    ));
    assert_eq!(seen.len(), 8);
    assert!(seen.windows(2).all(|pair| pair[0].cursor < pair[1].cursor));

    let sessions = client
        .list_sessions(ListSessionsRequest {
            machine: "desktop".into(),
            active_only: true,
        })
        .await
        .unwrap()
        .into_inner()
        .sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state(), SessionState::Running);

    server.stop().await;
}

#[tokio::test]
async fn reconnecting_daemons_replay_without_duplicates() {
    let postgres = Postgres::start().await;
    let server = TestServer::start(config(&postgres.new_database().await)).await;
    let user = server.user_token().await;
    let machine = server.pair(&user, "desktop").await;
    let mut client = server.client(&user).await;

    let mut daemon = FakeDaemon::attach(&server, &machine, vec![]).await.unwrap();
    let created = client
        .create_session(CreateSessionRequest {
            machine: "desktop".into(),
            instructions: "work".into(),
            title: "Work".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let s = created.session.unwrap().id;
    let turn = created.turn_id;
    daemon.command().await;
    daemon.command().await;
    daemon.event(&s, 1, "", state(SessionState::Starting)).await;
    daemon.event(&s, 2, &turn, started(&turn)).await;
    // Sent but the connection drops before it is acknowledged.
    daemon
        .send_event(&s, 3, &turn, state(SessionState::Running))
        .await;
    drop(daemon);

    let snapshot = vec![SessionSnapshot {
        session_id: s.clone(),
        claude_session_id: String::new(),
        state: SessionState::Running.into(),
        last_seq: 4,
    }];
    let mut daemon = FakeDaemon::attach(&server, &machine, snapshot)
        .await
        .unwrap();
    let acked = daemon.welcome.acked_seqs[&s];
    assert!(acked == 2 || acked == 3, "acked {acked}");
    // The daemon replays from the ack; anything the server already has is ignored.
    for seq in 2..=3 {
        let kind = if seq == 2 {
            started(&turn)
        } else {
            state(SessionState::Running)
        };
        daemon.event(&s, seq, &turn, kind).await;
    }
    daemon.event(&s, 4, &turn, completed(&turn)).await;

    let events = client
        .get_session(GetSessionRequest { session_id: s })
        .await
        .unwrap()
        .into_inner()
        .events;
    assert_eq!(events.len(), 4);
    let turn = &events.last().unwrap();
    assert!(matches!(session_kind(turn), Some(Kind::TurnCompleted(_))));

    server.stop().await;
}

#[tokio::test]
async fn reconnect_resends_unstarted_turns_and_fails_lost_sessions() {
    let postgres = Postgres::start().await;
    let server = TestServer::start(config(&postgres.new_database().await)).await;
    let user = server.user_token().await;
    let machine = server.pair(&user, "desktop").await;
    let mut client = server.client(&user).await;

    let mut daemon = FakeDaemon::attach(&server, &machine, vec![]).await.unwrap();
    let create = |title: &'static str| {
        let mut client = client.clone();
        async move {
            client
                .create_session(CreateSessionRequest {
                    machine: "desktop".into(),
                    instructions: "work".into(),
                    title: title.into(),
                })
                .await
                .unwrap()
                .into_inner()
        }
    };
    let kept = create("Kept").await;
    let kept_id = kept.session.unwrap().id;
    let start = daemon.command().await;
    daemon.command().await;
    daemon.result(&start.command_id, true, "").await;
    daemon
        .event(&kept_id, 1, "", state(SessionState::Starting))
        .await;

    let lost = create("Lost").await;
    let lost_id = lost.session.unwrap().id;
    daemon.command().await;
    daemon.command().await;
    daemon
        .event(&lost_id, 1, "", state(SessionState::Starting))
        .await;
    drop(daemon);

    // The daemon comes back knowing only one session, which hasn't started its turn.
    let snapshot = vec![SessionSnapshot {
        session_id: kept_id.clone(),
        claude_session_id: String::new(),
        state: SessionState::Starting.into(),
        last_seq: 1,
    }];
    let mut daemon = FakeDaemon::attach(&server, &machine, snapshot)
        .await
        .unwrap();
    match daemon.command().await.command {
        Some(command::Command::SendPrompt(prompt)) => assert_eq!(prompt.turn_id, kept.turn_id),
        other => panic!("expected the unstarted turn again, got {other:?}"),
    }
    daemon.no_command().await;

    let lost = client
        .get_session(GetSessionRequest {
            session_id: lost_id,
        })
        .await
        .unwrap()
        .into_inner();
    let session = lost.session.unwrap();
    assert_eq!(session.state(), SessionState::Failed);
    assert_eq!(session.current_turn.unwrap().state(), TurnState::Failed);
    assert!(lost.events.iter().any(|event| matches!(
        session_kind(event),
        Some(Kind::TurnFailed(failed)) if failed.reason == "daemon lost session"
    )));

    // A rejected prompt fails its turn and frees the session.
    let turn = client
        .send_prompt(SendPromptRequest {
            session_id: kept_id.clone(),
            instructions: "more".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .turn_id;
    daemon.no_command().await;
    daemon
        .result(&format!("prompt:{}", kept.turn_id), false, "bridge missing")
        .await;
    match daemon.command().await.command {
        Some(command::Command::SendPrompt(prompt)) => assert_eq!(prompt.turn_id, turn),
        other => panic!("expected the next turn, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn ambiguous_machine_names_list_candidates() {
    let postgres = Postgres::start().await;
    let server = TestServer::start(config(&postgres.new_database().await)).await;
    let user = server.user_token().await;
    server.pair(&user, "desktop").await;
    server.pair(&user, "desktop-2").await;
    server.pair(&user, "laptop").await;
    let mut client = server.client(&user).await;
    let create = |machine: &str| CreateSessionRequest {
        machine: machine.into(),
        instructions: "work".into(),
        title: "Work".into(),
    };

    // An exact name wins over prefixes; a unique prefix works.
    client.create_session(create("DESKTOP")).await.unwrap();
    client.create_session(create("lap")).await.unwrap();
    let status = client.create_session(create("desk")).await.unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition);
    let details = RpcStatus::decode(status.details()).unwrap();
    let target = AmbiguousTarget::decode(details.details[0].value.as_slice()).unwrap();
    assert_eq!(target.candidates, ["desktop", "desktop-2"]);
    let missing = client.create_session(create("server")).await.unwrap_err();
    assert_eq!(missing.code(), Code::NotFound);
    let untitled = client
        .create_session(CreateSessionRequest {
            title: " ".into(),
            ..create("laptop")
        })
        .await
        .unwrap_err();
    assert_eq!(untitled.code(), Code::InvalidArgument);

    server.stop().await;
}

#[derive(Clone, PartialEq, Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}

#[tokio::test]
async fn bootstrap_needs_only_createrole_and_createdb() {
    let postgres = Postgres::start().await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let admin_role = format!("limited_admin_{suffix}");
    let mut superuser = sqlx::PgPool::connect(&postgres.admin_url).await.unwrap();
    sqlx::query(&format!(
        "CREATE ROLE {admin_role} LOGIN PASSWORD 'admin' CREATEROLE CREATEDB"
    ))
    .execute(&superuser)
    .await
    .unwrap();
    superuser.close().await;
    let _ = &mut superuser;

    let admin_url = common::with_credentials(&postgres.admin_url, &admin_role, "admin", "postgres");
    let app_role = format!("blue_app_{suffix}");
    let app_url = common::with_credentials(&postgres.admin_url, &app_role, "first", &app_role);
    blue_server::db::bootstrap(&admin_url, &app_url)
        .await
        .unwrap();
    // Rerunning is safe and rotates the password.
    let app_url = common::with_credentials(&postgres.admin_url, &app_role, "second", &app_role);
    blue_server::db::bootstrap(&admin_url, &app_url)
        .await
        .unwrap();
    blue_server::db::migrate(&app_url).await.unwrap();

    let app = blue_server::db::connect(&app_url).await.unwrap();
    assert!(blue_server::db::migrations_current(&app).await.unwrap());
    // Other roles get nothing in the public schema.
    let public_create: bool =
        sqlx::query_scalar("SELECT has_schema_privilege('public', 'public', 'CREATE')")
            .fetch_one(&app)
            .await
            .unwrap();
    assert!(!public_create);
}

/// A stand-in for github.com and api.github.com: the device flow says "pending" once, then
/// hands out a token for the GitHub user with id `user_id`.
async fn fake_github(user_id: i64) -> String {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::routing::{get, post};
    use serde_json::json;

    let polls = Arc::new(AtomicUsize::new(0));
    let app = axum::Router::new()
        .route(
            "/login/device/code",
            post(|| async {
                Json(json!({
                    "device_code": "device-123",
                    "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 60,
                    "interval": 1,
                }))
            }),
        )
        .route(
            "/login/oauth/access_token",
            post(move || {
                let polls = polls.clone();
                async move {
                    if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Json(json!({ "error": "authorization_pending" }))
                    } else {
                        Json(json!({ "access_token": "gho_test", "token_type": "bearer" }))
                    }
                }
            }),
        )
        .route(
            "/user",
            get(move || async move {
                Json(json!({ "id": user_id, "login": "octocat", "name": "Mona", "avatar_url": "" }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn github_sign_in_checks_the_allowlist() {
    use blue_protocol::auth::v1::{LoginWithGitHubRequest, login_with_git_hub_response::Step};

    let postgres = Postgres::start().await;
    let database = postgres.new_database().await;
    for (github_id, allowed) in [(42, true), (7, false)] {
        let github = fake_github(github_id).await;
        let mut config = config(&database);
        config.github.client_id = Some("client-id".into());
        config.github.web_url = github.clone();
        config.github.api_url = github;
        let server = TestServer::start(config).await;

        let mut login = server
            .auth(None)
            .await
            .login_with_git_hub(LoginWithGitHubRequest {
                client_name: "test".into(),
            })
            .await
            .unwrap()
            .into_inner();
        match login.message().await.unwrap().unwrap().step {
            Some(Step::DeviceCode(code)) => assert_eq!(code.user_code, "ABCD-1234"),
            other => panic!("expected a device code, got {other:?}"),
        }
        let result = login.message().await;
        if allowed {
            let Some(Step::Complete(done)) = result.unwrap().unwrap().step else {
                panic!("expected LoginComplete");
            };
            assert_eq!(done.user.unwrap().github_login, "octocat");
            let user = server
                .auth(Some(&done.session_token))
                .await
                .get_current_user(GetCurrentUserRequest {})
                .await
                .unwrap()
                .into_inner()
                .user
                .unwrap();
            assert_eq!(user.name, "Mona");
            // Logging out revokes the token immediately.
            server
                .auth(Some(&done.session_token))
                .await
                .logout(blue_protocol::auth::v1::LogoutRequest {})
                .await
                .unwrap();
            let after = server
                .auth(Some(&done.session_token))
                .await
                .get_current_user(GetCurrentUserRequest {})
                .await;
            assert_eq!(after.unwrap_err().code(), Code::Unauthenticated);
        } else {
            assert_eq!(result.unwrap_err().code(), Code::PermissionDenied);
        }
        server.stop().await;
    }
}
