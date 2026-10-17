use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use blue_common::Bearer;
use blue_protocol::bridge::v1::{Close, Ready, bridge_to_daemon, daemon_to_bridge};
use blue_protocol::daemon::v1::GetMachineRequest;
use blue_protocol::daemon::v1::daemon_service_client::DaemonServiceClient;
use tokio::time::timeout;

use crate::bridge_proc::BridgeProcess;
use crate::config::DaemonConfig;

const BRIDGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Where to find the bridge and `claude`; shared by blue-daemon and blue-daemon-test.
#[derive(clap::Args)]
pub struct ExecutablePaths {
    /// Path to the claude-bridge executable. Defaults to the one next to this binary.
    #[arg(long, env = "BLUE_BRIDGE_PATH")]
    pub bridge: Option<PathBuf>,
    /// Path to the installed `claude` executable. Defaults to `claude` on PATH.
    #[arg(long, env = "BLUE_CLAUDE_PATH")]
    pub claude: Option<PathBuf>,
}

impl ExecutablePaths {
    pub fn bridge(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.bridge {
            return Ok(path.clone());
        }
        let exe = std::env::current_exe().context("locate own executable")?;
        Ok(exe.with_file_name(format!("claude-bridge{}", std::env::consts::EXE_SUFFIX)))
    }

    /// Fills in paths not given as flags from daemon.toml.
    pub fn or_config(mut self, config: Option<&DaemonConfig>) -> Self {
        if let Some(config) = config {
            self.bridge = self.bridge.or_else(|| config.bridge_path());
            self.claude = self.claude.or_else(|| config.claude_path());
        }
        self
    }
}

/// The server this machine is paired with, for checking the token.
pub struct ServerCheck {
    pub url: String,
    pub token: String,
}

/// Checks everything a session needs, prints one line per check, and fails if any check failed.
pub async fn run(
    bridge: &Path,
    claude: Option<&Path>,
    server: Option<ServerCheck>,
) -> anyhow::Result<()> {
    let mut ok = true;

    match crate::claude::resolve(claude).await {
        Ok(install) => println!(
            "claude  ok    {} ({})",
            install.version,
            install.path.display()
        ),
        Err(err) => {
            ok = false;
            println!("claude  FAIL  {err:#}");
        }
    }

    match timeout(BRIDGE_TIMEOUT, bridge_handshake(bridge)).await {
        Ok(Ok(ready)) => println!(
            "bridge  ok    {} (bridge {}, agent sdk {}, bun {})",
            bridge.display(),
            ready.bridge_version,
            ready.sdk_version,
            ready.runtime_version
        ),
        Ok(Err(err)) => {
            ok = false;
            println!("bridge  FAIL  {err:#}");
        }
        Err(_) => {
            ok = false;
            println!("bridge  FAIL  handshake timed out after {BRIDGE_TIMEOUT:?}");
        }
    }

    match server {
        None => println!("server -     not paired yet (blue-daemon pair)"),
        Some(server) => match timeout(BRIDGE_TIMEOUT, check_server(&server)).await {
            Ok(Ok(name)) => println!("server  ok    {} (paired as {name})", server.url),
            Ok(Err(err)) => {
                ok = false;
                println!("server  FAIL  {}: {err:#}", server.url);
            }
            Err(_) => {
                ok = false;
                println!("server  FAIL  {}: timed out", server.url);
            }
        },
    }

    if !ok {
        bail!("this machine is not ready to run Blue sessions");
    }
    Ok(())
}

/// Checks that the server is reachable and accepts the machine token; returns the machine name.
async fn check_server(server: &ServerCheck) -> anyhow::Result<String> {
    let channel = blue_common::connect(&server.url).await?;
    let mut client = DaemonServiceClient::with_interceptor(channel, Bearer::new(&server.token)?);
    let machine = client
        .get_machine(GetMachineRequest {})
        .await
        .context("machine token rejected")?
        .into_inner();
    Ok(machine.machine_name)
}

/// Spawns the bridge, waits for `Ready`, then asks it to exit with `Close`.
pub async fn bridge_handshake(path: &Path) -> anyhow::Result<Ready> {
    let (mut bridge, ready) = BridgeProcess::spawn(path, None).await?;
    bridge
        .send(daemon_to_bridge::Command::Close(Close {}))
        .await?;
    match bridge
        .next_event()
        .await
        .context("bridge closed stdout")??
    {
        bridge_to_daemon::Event::Exited(exited) if exited.error.is_empty() => {}
        other => bail!("expected a clean Exited after Close, got {other:?}"),
    }
    let status = bridge.wait().await?;
    if !status.success() {
        bail!("bridge exited with {status}");
    }
    Ok(ready)
}
