use std::path::PathBuf;

use blue_daemon::doctor;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "blue-daemon",
    version,
    about = "Connects this machine to the Blue server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the bridge and `claude` are usable from this machine.
    Doctor {
        /// Path to the claude-bridge executable. Defaults to the one next to blue-daemon.
        #[arg(long, env = "BLUE_BRIDGE_PATH")]
        bridge: Option<PathBuf>,
        /// Path to the installed `claude` executable. Defaults to `claude` on PATH.
        #[arg(long, env = "BLUE_CLAUDE_PATH")]
        claude: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Doctor { bridge, claude } => {
            let bridge = match bridge {
                Some(path) => path,
                None => doctor::default_bridge_path()?,
            };
            doctor::run(&bridge, claude.as_deref()).await
        }
    }
}
