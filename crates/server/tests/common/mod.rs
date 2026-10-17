//! Test harness: a real Postgres, a bootstrapped and migrated database per test, the server on
//! a random port, and a fake daemon speaking the real protocol.
//!
//! Postgres comes from `BLUE_TEST_DATABASE_URL` (an admin DSN, as in CI) or, if that's unset, a
//! throwaway server started with postgresql_embedded (downloaded once into ~/.theseus).

#![allow(dead_code)]

use std::time::Duration;

use blue_common::Bearer;
use blue_protocol::PROTOCOL_VERSION;
use blue_protocol::auth::v1::auth_service_client::AuthServiceClient;
use blue_protocol::auth::v1::{ClaimPairingCodeRequest, CreatePairingCodeRequest};
use blue_protocol::client::v1::client_service_client::ClientServiceClient;
use blue_protocol::client::v1::{Event, SubscribeRequest};
use blue_protocol::daemon::v1::daemon_service_client::DaemonServiceClient;
use blue_protocol::daemon::v1::{
    AttachRequest, AttachResponse, Command, CommandResult, DaemonEvent, EventsAck, Hello,
    SessionSnapshot, Welcome, attach_request, attach_response,
};
use blue_protocol::events::v1::{SessionEvent, session_event::Kind};
use blue_server::config::{GitHubConfig, LiveKitConfig, ServeConfig};
use blue_server::github::GitHubUser;
use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tonic::Streaming;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

pub const WAIT: Duration = Duration::from_secs(20);

pub type Client = ClientServiceClient<InterceptedService<Channel, Bearer>>;
pub type Auth = AuthServiceClient<InterceptedService<Channel, Bearer>>;

/// A Postgres server; dropping it stops an embedded one.
pub struct Postgres {
    _embedded: Option<PostgreSQL>,
    pub admin_url: String,
}

impl Postgres {
    pub async fn start() -> Self {
        if let Ok(admin_url) = std::env::var("BLUE_TEST_DATABASE_URL") {
            return Self {
                _embedded: None,
                admin_url,
            };
        }
        let settings = Settings {
            version: VersionReq::parse("=17").unwrap(),
            timeout: Some(Duration::from_secs(120)),
            ..Settings::default()
        };
        let mut postgres = PostgreSQL::new(settings);
        postgres.setup().await.expect("install Postgres");
        postgres.start().await.expect("start Postgres");
        let admin_url = postgres.settings().url("postgres");
        Self {
            _embedded: Some(postgres),
            admin_url,
        }
    }

    /// A fresh app role + database, bootstrapped and migrated; returns its DSN.
    pub async fn new_database(&self) -> String {
        let name = format!("blue_test_{}", uuid::Uuid::new_v4().simple());
        let app_url = with_credentials(&self.admin_url, &name, "app-pass'word", &name);
        blue_server::db::bootstrap(&self.admin_url, &app_url)
            .await
            .expect("bootstrap");
        blue_server::db::migrate(&app_url).await.expect("migrate");
        app_url
    }
}

/// `base` with its user, password and database replaced.
pub fn with_credentials(base: &str, user: &str, password: &str, database: &str) -> String {
    let mut url = url::Url::parse(base).unwrap();
    url.set_username(user).unwrap();
    url.set_password(Some(password)).unwrap();
    url.set_path(&format!("/{database}"));
    url.to_string()
}

pub fn config(database_url: &str) -> ServeConfig {
    ServeConfig {
        database_url: database_url.into(),
        voice_token: Some("voice-secret".into()),
        github: GitHubConfig {
            client_id: None,
            allowed_user_ids: vec![42],
            web_url: "https://github.com".into(),
            api_url: "https://api.github.com".into(),
        },
        session_ttl: Duration::from_secs(3600),
        livekit: LiveKitConfig {
            url: None,
            api_key: None,
            api_secret: None,
            agent_name: "blue-voice".into(),
        },
    }
}

/// The server running in this process.
pub struct TestServer {
    pub url: String,
    pub db: PgPool,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl TestServer {
    pub async fn start(config: ServeConfig) -> Self {
        let db = blue_server::db::connect(&config.database_url)
            .await
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(blue_server::serve(config, listener, async {
            let _ = stopped.await;
        }));
        Self {
            url,
            db,
            stop: Some(stop),
            task,
        }
    }

    pub async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout(WAIT, self.task)
            .await
            .expect("server stops promptly")
            .unwrap()
            .unwrap();
    }

    pub async fn channel(&self) -> Channel {
        blue_common::connect(&self.url).await.unwrap()
    }

    /// A signed-in user's session token (GitHub sign-in itself is tested separately).
    pub async fn user_token(&self) -> String {
        let user = GitHubUser {
            id: 42,
            login: "octocat".into(),
            name: None,
            avatar_url: String::new(),
        };
        blue_server::services::auth::create_session(
            &self.db,
            Duration::from_secs(3600),
            &user,
            "test",
        )
        .await
        .unwrap()
        .0
    }

    pub async fn client(&self, token: &str) -> Client {
        ClientServiceClient::with_interceptor(self.channel().await, Bearer::new(token).unwrap())
    }

    pub async fn auth(&self, token: Option<&str>) -> Auth {
        let bearer = match token {
            Some(token) => Bearer::new(token).unwrap(),
            None => Bearer::none(),
        };
        AuthServiceClient::with_interceptor(self.channel().await, bearer)
    }

    /// Pairs a machine; returns its token.
    pub async fn pair(&self, user_token: &str, name: &str) -> String {
        let code = self
            .auth(Some(user_token))
            .await
            .create_pairing_code(CreatePairingCodeRequest {})
            .await
            .unwrap()
            .into_inner()
            .code;
        self.auth(None)
            .await
            .claim_pairing_code(ClaimPairingCodeRequest {
                code,
                machine_name: name.into(),
            })
            .await
            .unwrap()
            .into_inner()
            .machine_token
    }

    pub async fn subscribe(
        &self,
        token: &str,
        cursor: Option<u64>,
    ) -> Streaming<blue_protocol::client::v1::SubscribeResponse> {
        self.client(token)
            .await
            .subscribe(SubscribeRequest {
                cursor,
                session_ids: Vec::new(),
                include_raw: true,
            })
            .await
            .unwrap()
            .into_inner()
    }
}

/// A daemon driven by hand.
pub struct FakeDaemon {
    tx: mpsc::Sender<AttachRequest>,
    inbound: Streaming<AttachResponse>,
    pub welcome: Welcome,
}

impl FakeDaemon {
    pub async fn attach(
        server: &TestServer,
        token: &str,
        sessions: Vec<SessionSnapshot>,
    ) -> Result<Self, tonic::Status> {
        let mut client = DaemonServiceClient::with_interceptor(
            server.channel().await,
            Bearer::new(token).unwrap(),
        );
        let (tx, rx) = mpsc::channel(64);
        tx.send(AttachRequest {
            message: Some(attach_request::Message::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                versions: None,
                os: "test".into(),
                arch: "test".into(),
                sessions,
            })),
        })
        .await
        .unwrap();
        let mut inbound = client
            .attach(tokio_stream::wrappers::ReceiverStream::new(rx))
            .await?
            .into_inner();
        let welcome = match inbound.message().await? {
            Some(AttachResponse {
                message: Some(attach_response::Message::Welcome(welcome)),
            }) => welcome,
            other => panic!("expected Welcome, got {other:?}"),
        };
        Ok(Self {
            tx,
            inbound,
            welcome,
        })
    }

    async fn next(&mut self) -> Option<attach_response::Message> {
        timeout(WAIT, self.inbound.message())
            .await
            .expect("timed out waiting for the server")
            .ok()
            .flatten()
            .and_then(|response| response.message)
    }

    pub async fn command(&mut self) -> Command {
        loop {
            match self.next().await {
                Some(attach_response::Message::Command(command)) => return command,
                Some(_) => {}
                None => panic!("stream ended while waiting for a command"),
            }
        }
    }

    /// Asserts no command arrives for a moment (acks are skipped).
    pub async fn no_command(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        loop {
            match tokio::time::timeout_at(deadline, self.inbound.message()).await {
                Err(_) => return,
                Ok(Ok(Some(AttachResponse {
                    message: Some(attach_response::Message::Command(command)),
                }))) => panic!("unexpected command {command:?}"),
                Ok(Ok(Some(_))) => {}
                Ok(other) => panic!("stream ended: {other:?}"),
            }
        }
    }

    /// Sends an event and waits for its ack.
    pub async fn event(&mut self, session_id: &str, seq: u64, turn_id: &str, kind: Kind) {
        self.send_event(session_id, seq, turn_id, kind).await;
        loop {
            match self.next().await {
                Some(attach_response::Message::EventsAck(EventsAck {
                    session_id: acked,
                    up_to_seq,
                })) if acked == session_id && up_to_seq == seq => return,
                Some(attach_response::Message::Command(command)) => {
                    panic!("unexpected command while waiting for an ack: {command:?}")
                }
                Some(_) => {}
                None => panic!("stream ended while waiting for an ack"),
            }
        }
    }

    pub async fn send_event(&mut self, session_id: &str, seq: u64, turn_id: &str, kind: Kind) {
        self.tx
            .send(AttachRequest {
                message: Some(attach_request::Message::Event(DaemonEvent {
                    session_id: session_id.into(),
                    seq,
                    turn_id: turn_id.into(),
                    occurred_at: None,
                    event: Some(SessionEvent { kind: Some(kind) }),
                })),
            })
            .await
            .unwrap();
    }

    pub async fn result(&mut self, command_id: &str, ok: bool, message: &str) {
        self.tx
            .send(AttachRequest {
                message: Some(attach_request::Message::CommandResult(CommandResult {
                    command_id: command_id.into(),
                    ok,
                    error_code: String::new(),
                    error_message: message.into(),
                })),
            })
            .await
            .unwrap();
    }

    /// Waits for the server to end the stream.
    pub async fn closed(mut self) -> Option<tonic::Status> {
        loop {
            match timeout(WAIT, self.inbound.message())
                .await
                .expect("stream should end")
            {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(status) => return Some(status),
            }
        }
    }
}

/// Reads events until one matches; returns all of them.
pub async fn events_until(
    stream: &mut Streaming<blue_protocol::client::v1::SubscribeResponse>,
    done: impl Fn(&Event) -> bool,
) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = timeout(WAIT, stream.message())
            .await
            .unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"))
            .unwrap()
            .expect("subscription ended")
            .event
            .unwrap();
        let stop = done(&event);
        seen.push(event);
        if stop {
            return seen;
        }
    }
}

pub fn session_kind(event: &Event) -> Option<&Kind> {
    match event.kind.as_ref()? {
        blue_protocol::client::v1::event::Kind::Session(envelope) => {
            envelope.event.as_ref()?.kind.as_ref()
        }
        _ => None,
    }
}
