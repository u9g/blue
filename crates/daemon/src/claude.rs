//! Locates the machine's installed Claude Code. Blue never bundles Claude: the machine must
//! already have `claude` installed and logged in, and the daemon refuses to run without it.
//! The resolved path is passed to the bridge, so the Agent SDK never falls back to its own copy.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use tokio::process::Command;

const INSTALL_HINT: &str = "Blue needs Claude Code installed on this machine. Install it \
(https://code.claude.com/docs/en/setup), run `claude` once to log in, then make \
sure it is on PATH or set BLUE_CLAUDE_PATH.";

pub struct ClaudeInstall {
    pub path: PathBuf,
    pub version: String,
}

/// Resolves `claude` from an explicit path (flag / `BLUE_CLAUDE_PATH`) or from PATH, and checks
/// that it runs.
pub async fn resolve(explicit: Option<&Path>) -> anyhow::Result<ClaudeInstall> {
    let path = match explicit {
        Some(path) => path.to_owned(),
        None => which::which("claude")
            .map_err(|_| anyhow!("`claude` was not found on PATH. {INSTALL_HINT}"))?,
    };
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .await
        .map_err(|err| anyhow!("could not run {} ({err}). {INSTALL_HINT}", path.display()))?;
    if !output.status.success() {
        bail!(
            "{} --version exited with {}. {INSTALL_HINT}",
            path.display(),
            output.status
        );
    }
    Ok(ClaudeInstall {
        path,
        version: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    })
}
