use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use blue_protocol::PROTOCOL_VERSION;
use blue_protocol::bridge::v1::{
    BridgeInput, BridgeOutput, Close, Ready, bridge_input, bridge_output,
};
use blue_protocol::framing::{read_frame, write_frame};
use tokio::io::{AsyncRead, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

const BRIDGE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn default_bridge_path() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("locate blue-daemon executable")?;
    Ok(exe.with_file_name(format!("claude-bridge{}", std::env::consts::EXE_SUFFIX)))
}

pub async fn run(bridge: &Path, claude: &Path) -> anyhow::Result<()> {
    let ready = timeout(BRIDGE_TIMEOUT, bridge_handshake(bridge))
        .await
        .context("bridge handshake timed out")??;
    println!(
        "bridge  ok    {} (bridge {}, agent sdk {}, bun {})",
        bridge.display(),
        ready.bridge_version,
        ready.sdk_version,
        ready.runtime_version
    );

    match claude_version(claude).await {
        Ok(version) => println!("claude  ok    {version}"),
        Err(err) => println!("claude  FAIL  {err:#}"),
    }
    Ok(())
}

/// Spawns the bridge, waits for `Ready`, then asks it to exit with `Close`.
async fn bridge_handshake(bridge: &Path) -> anyhow::Result<Ready> {
    let mut child = Command::new(bridge)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn bridge {}", bridge.display()))?;
    let mut stdin = child.stdin.take().context("bridge stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().context("bridge stdout")?);

    let ready = match next_event(&mut stdout).await? {
        bridge_output::Event::Ready(ready) => ready,
        other => bail!("expected Ready as the bridge's first frame, got {other:?}"),
    };
    if ready.protocol_version != PROTOCOL_VERSION {
        bail!(
            "bridge speaks protocol v{}, daemon speaks v{PROTOCOL_VERSION}",
            ready.protocol_version
        );
    }

    let close = BridgeInput {
        command: Some(bridge_input::Command::Close(Close {})),
    };
    write_frame(&mut stdin, &close).await?;
    match next_event(&mut stdout).await? {
        bridge_output::Event::Exited(exited) if exited.code == 0 => {}
        other => bail!("expected Exited with code 0 after Close, got {other:?}"),
    }

    let status = child.wait().await?;
    if !status.success() {
        bail!("bridge exited with {status}");
    }
    Ok(ready)
}

async fn next_event<R: AsyncRead + Unpin>(stdout: &mut R) -> anyhow::Result<bridge_output::Event> {
    read_frame::<_, BridgeOutput>(stdout)
        .await?
        .context("bridge closed stdout")?
        .event
        .context("bridge sent an empty frame")
}

async fn claude_version(claude: &Path) -> anyhow::Result<String> {
    let output = Command::new(claude)
        .arg("--version")
        .output()
        .await
        .with_context(|| format!("run {} --version", claude.display()))?;
    if !output.status.success() {
        bail!(
            "{} --version exited with {}",
            claude.display(),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
