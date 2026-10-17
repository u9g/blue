use clap::{Parser, Subcommand};
use tonic::transport::Channel;
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

#[derive(Parser)]
#[command(name = "blue", version, about = "Text client for the Blue hub")]
struct Cli {
    /// Hub gRPC endpoint.
    #[arg(
        long,
        global = true,
        env = "BLUE_HUB_URL",
        default_value = "http://127.0.0.1:50051"
    )]
    hub: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the hub is up (gRPC health check).
    Health,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Health => health(cli.hub).await,
    }
}

async fn health(hub: String) -> anyhow::Result<()> {
    let channel = Channel::from_shared(hub.clone())?.connect().await?;
    let mut client = HealthClient::new(channel);
    let response = client
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await?
        .into_inner();
    println!("{hub}: {}", response.status().as_str_name());
    Ok(())
}
