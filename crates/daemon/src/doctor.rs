use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use blue_protocol::bridge::v1::{Close, Ready, bridge_to_daemon, daemon_to_bridge};
use tokio::time::timeout;

use crate::bridge_proc::BridgeProcess;

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
}

/// Checks everything a session needs, prints one line per check, and fails if any check failed.
pub async fn run(bridge: &Path, claude: Option<&Path>) -> anyhow::Result<()> {
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

    if !ok {
        bail!("this machine is not ready to run Blue sessions");
    }
    Ok(())
}

/// Spawns the bridge, waits for `Ready`, then asks it to exit with `Close`.
async fn bridge_handshake(path: &Path) -> anyhow::Result<Ready> {
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
