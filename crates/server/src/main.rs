use std::net::SocketAddr;
use std::time::Duration;

use blue_server::config::ServeConfig;
use blue_server::github::GitHubUser;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "blue-server", version, about = "Blue management server")]
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
        #[command(flatten)]
        config: Box<ServeConfig>,
    },
    /// Database setup.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Issue a session token for a GitHub account directly, bypassing GitHub sign-in. For local
    /// development and first-time setup; needs the database credentials.
    Token {
        #[arg(long, env = "BLUE_DATABASE_URL", hide_env_values = true)]
        database_url: String,
        /// GitHub numeric user id to sign in as.
        #[arg(long)]
        github_id: i64,
        /// GitHub login to record for a new user.
        #[arg(long, default_value = "dev")]
        github_login: String,
        #[arg(long, default_value = "30d", value_parser = humantime::parse_duration)]
        ttl: Duration,
    },
}

#[derive(Subcommand)]
enum DbCommand {
    /// Create the app role and database using an admin DSN (needs CREATEROLE and CREATEDB).
    /// Idempotent; also resets the role's password to the one in BLUE_DATABASE_URL.
    Bootstrap {
        #[arg(long, env = "BLUE_DATABASE_ADMIN_URL", hide_env_values = true)]
        admin_url: String,
        #[arg(long, env = "BLUE_DATABASE_URL", hide_env_values = true)]
        database_url: String,
    },
    /// Apply migrations as the app role.
    Migrate {
        #[arg(long, env = "BLUE_DATABASE_URL", hide_env_values = true)]
        database_url: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Serve { listen, config } => {
            let listener = TcpListener::bind(listen).await?;
            info!(%listen, "blue-server listening");
            blue_server::serve(*config, listener, shutdown_signal()).await?;
            info!("blue-server stopped");
            Ok(())
        }
        Command::Db {
            command:
                DbCommand::Bootstrap {
                    admin_url,
                    database_url,
                },
        } => blue_server::db::bootstrap(&admin_url, &database_url).await,
        Command::Db {
            command: DbCommand::Migrate { database_url },
        } => {
            blue_server::db::migrate(&database_url).await?;
            info!("migrations applied");
            Ok(())
        }
        Command::Token {
            database_url,
            github_id,
            github_login,
            ttl,
        } => {
            let db = blue_server::db::connect(&database_url).await?;
            let user = GitHubUser {
                id: github_id,
                login: github_login,
                name: None,
                avatar_url: String::new(),
            };
            let (token, _, _) =
                blue_server::services::auth::create_session(&db, ttl, &user, "blue-server token")
                    .await
                    .map_err(|status| anyhow::anyhow!("{}", status.message()))?;
            println!("{token}");
            Ok(())
        }
    }
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
