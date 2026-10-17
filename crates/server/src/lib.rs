//! blue-server: authenticates users, the voice agent and daemons; stores sessions, turns and
//! events in Postgres; routes work to daemons and fans events out to clients.

pub mod auth;
pub mod config;
pub mod db;
pub mod github;
pub mod hub;
pub mod livekit;
pub mod services;
pub mod time;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use blue_protocol::auth::v1::auth_service_server::AuthServiceServer;
use blue_protocol::client::v1::client_service_server::ClientServiceServer;
use blue_protocol::daemon::v1::daemon_service_server::DaemonServiceServer;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;

use crate::auth::{Authenticator, RateLimit};
use crate::config::ServeConfig;
use crate::github::GitHub;
use crate::hub::Hub;

pub struct AppState {
    pub db: PgPool,
    pub config: ServeConfig,
    pub auth: Authenticator,
    pub hub: Hub,
    pub github: Option<GitHub>,
    pub login_limit: RateLimit,
    pub claim_limit: RateLimit,
    /// Cancelled when the server begins shutting down, to end long-lived streams.
    pub shutdown: CancellationToken,
}

const SERVICES: &[&str] = &[
    "blue.auth.v1.AuthService",
    "blue.client.v1.ClientService",
    "blue.daemon.v1.DaemonService",
];

/// Serves every gRPC service on `listener` until `shutdown` resolves. The database must already
/// be migrated.
pub async fn serve(
    config: ServeConfig,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let db = db::connect(&config.database_url).await?;
    let state = Arc::new(AppState {
        auth: Authenticator::new(db.clone(), config.voice_token.as_deref()),
        hub: Hub::new(db.clone()).await?,
        github: GitHub::new(&config.github),
        login_limit: RateLimit::new(10, Duration::from_secs(60)),
        claim_limit: RateLimit::new(10, Duration::from_secs(60)),
        shutdown: CancellationToken::new(),
        db,
        config,
    });
    if state.github.is_none() {
        tracing::warn!("BLUE_GITHUB_CLIENT_ID is not set; GitHub sign-in is disabled");
    }

    let (health, health_service) = tonic_health::server::health_reporter();
    let readiness = tokio::spawn(report_readiness(state.db.clone(), health));
    // Graceful shutdown waits for open streams, and daemon and subscriber streams never end on
    // their own: end them first.
    let shutdown = {
        let state = state.clone();
        async move {
            shutdown.await;
            state.shutdown.cancel();
        }
    };

    let result = Server::builder()
        .http2_keepalive_interval(Some(Duration::from_secs(20)))
        .add_service(health_service)
        .add_service(AuthServiceServer::new(services::auth::Auth(state.clone())))
        .add_service(ClientServiceServer::new(services::client::Clients(
            state.clone(),
        )))
        .add_service(DaemonServiceServer::new(services::daemon::Daemons(
            state.clone(),
        )))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown)
        .await;
    readiness.abort();
    result?;
    Ok(())
}

/// Ready means the database is reachable and fully migrated.
async fn report_readiness(db: PgPool, health: HealthReporter) {
    let mut last = None;
    loop {
        let ready = matches!(db::migrations_current(&db).await, Ok(true));
        if last != Some(ready) {
            let status = if ready {
                ServingStatus::Serving
            } else {
                ServingStatus::NotServing
            };
            if !ready {
                tracing::warn!("not ready: database unreachable or migrations pending");
            }
            for service in std::iter::once(&"").chain(SERVICES) {
                health.set_service_status(*service, status).await;
            }
            last = Some(ready);
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
