use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "blue-hub", version, about = "Blue management server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the gRPC server.
    Serve {
        #[arg(long, env = "BLUE_LISTEN", default_value = "0.0.0.0:50051")]
        listen: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Serve { listen } => serve(listen).await,
    }
}

async fn serve(listen: SocketAddr) -> anyhow::Result<()> {
    // The reporter starts with the overall ("") service SERVING; per-service status is
    // reported once the Blue services are registered.
    let (_health_reporter, health_service) = tonic_health::server::health_reporter();

    info!(%listen, "blue-hub listening");
    Server::builder()
        .add_service(health_service)
        .serve_with_shutdown(listen, shutdown_signal())
        .await?;
    info!("blue-hub stopped");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
