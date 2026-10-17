use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use blue_common::Bearer;
use blue_daemon::agent::{self, Launcher};
use blue_daemon::claude::ClaudeInstall;
use blue_daemon::config::{self, DaemonConfig, DirArgs};
use blue_daemon::connection::ConnectionOptions;
use blue_daemon::doctor::{self, ExecutablePaths, ServerCheck};
use blue_daemon::store::Store;
use blue_protocol::auth::v1::ClaimPairingCodeRequest;
use blue_protocol::auth::v1::auth_service_client::AuthServiceClient;
use blue_protocol::daemon::v1::Versions;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "blue-daemon",
    version,
    about = "Connects this machine to the Blue server"
)]
struct Cli {
    #[command(flatten)]
    dir: DirArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair this machine with a server, using a code from `blue pair-code`.
    Pair {
        /// Server URL, e.g. https://blue.example.com.
        #[arg(long)]
        server: String,
        #[arg(long)]
        code: String,
        /// Name to register this machine under. Defaults to the host name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Connect to the server and run sessions, in the foreground.
    Start {
        #[command(flatten)]
        paths: ExecutablePaths,
        /// Play back this recording for every session instead of running Claude (testing).
        #[arg(long, hide = true)]
        replay: Option<PathBuf>,
    },
    /// Check that the bridge, `claude` and the server are usable from this machine.
    Doctor(ExecutablePaths),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    let dir = cli.dir.dir()?;
    let explicit_dir = cli.dir.dir.is_some();
    match cli.command {
        Command::Pair { server, code, name } => {
            let name = name.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into());
            let channel = blue_common::connect(&server).await?;
            let claimed = AuthServiceClient::with_interceptor(channel, Bearer::none())
                .claim_pairing_code(ClaimPairingCodeRequest {
                    code,
                    machine_name: name.clone(),
                })
                .await
                .map_err(|status| anyhow::anyhow!("pairing failed: {}", status.message()))?
                .into_inner();
            config::token_store(&dir, explicit_dir, &claimed.machine_id)
                .set(&claimed.machine_token)?;
            DaemonConfig {
                server_url: server,
                machine_name: name.clone(),
                machine_id: claimed.machine_id,
                claude_path: String::new(),
                bridge_path: String::new(),
                start_directory: "~".into(),
                forward_raw: true,
            }
            .save(&dir)?;
            println!(
                "Paired as {name:?}. Settings: {}\nStart the daemon with: blue-daemon start",
                config::config_path(&dir).display()
            );
            Ok(())
        }
        Command::Start { paths, replay } => {
            let config = DaemonConfig::load(&dir)?;
            let token = config::token_store(&dir, explicit_dir, &config.machine_id)
                .get()?
                .context("no machine token stored; pair this machine again")?;
            let paths = paths.or_config(Some(&config));
            let bridge_path = paths.bridge()?;
            let claude = match &replay {
                // Replay never runs Claude, so it doesn't need it installed.
                Some(_) => ClaudeInstall {
                    path: paths.claude.clone().unwrap_or_else(|| "claude".into()),
                    version: "replay".into(),
                },
                None => blue_daemon::claude::resolve(paths.claude.as_deref()).await?,
            };
            let ready = doctor::bridge_handshake(&bridge_path)
                .await
                .with_context(|| format!("bridge {} is not usable", bridge_path.display()))?;
            tracing::info!(
                machine = %config.machine_name,
                claude = %claude.version,
                bridge = %ready.bridge_version,
                "starting"
            );
            let connection = ConnectionOptions {
                server_url: config.server_url.clone(),
                token,
                versions: Versions {
                    daemon: env!("CARGO_PKG_VERSION").into(),
                    bridge: ready.bridge_version,
                    sdk: ready.sdk_version,
                    claude: claude.version,
                },
            };
            let launcher = Launcher {
                bridge_path,
                claude_path: claude.path,
                start_directory: config.start_directory()?,
                forward_raw: config.forward_raw,
                replay,
            };
            let store = Arc::new(Store::open(&config::state_path(&dir))?);
            agent::run(connection, launcher, store, async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        }
        Command::Doctor(paths) => {
            let config = DaemonConfig::load(&dir).ok();
            let server = match &config {
                Some(config) => config::token_store(&dir, explicit_dir, &config.machine_id)
                    .get()?
                    .map(|token| ServerCheck {
                        url: config.server_url.clone(),
                        token,
                    }),
                None => None,
            };
            let paths = paths.or_config(config.as_ref());
            doctor::run(&paths.bridge()?, paths.claude.as_deref(), server).await
        }
    }
}
