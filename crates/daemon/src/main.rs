mod doctor;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
        /// Path to the `claude` executable.
        #[arg(long, env = "BLUE_CLAUDE_PATH", default_value = "claude")]
        claude: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Doctor { bridge, claude } => {
            let bridge = match bridge {
                Some(path) => path,
                None => doctor::default_bridge_path()?,
            };
            doctor::run(&bridge, &claude).await
        }
    }
}
