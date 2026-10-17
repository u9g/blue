use blue_daemon::doctor::{self, ExecutablePaths};
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
    Doctor(ExecutablePaths),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match Cli::parse().command {
        Command::Doctor(paths) => doctor::run(&paths.bridge()?, paths.claude.as_deref()).await,
    }
}
