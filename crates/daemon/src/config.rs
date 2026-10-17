//! `daemon.toml` and the files next to it: the machine token (fallback location) and the
//! SQLite state. All live in one directory (`%APPDATA%\blue\` on Windows, `~/.config/blue/`
//! elsewhere), which `--dir` / `BLUE_DAEMON_DIR` can move, e.g. to run two daemons on one box.

use std::path::{Path, PathBuf};

use anyhow::Context;
use blue_common::credentials::CredentialStore;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonConfig {
    pub server_url: String,
    pub machine_name: String,
    pub machine_id: String,
    /// Installed `claude`; empty means `claude` on PATH.
    #[serde(default)]
    pub claude_path: String,
    /// claude-bridge executable; empty means next to blue-daemon.
    #[serde(default)]
    pub bridge_path: String,
    /// Where scouting starts for new sessions; `~` is the home directory.
    #[serde(default = "default_start_directory")]
    pub start_directory: String,
    /// Also send every raw SDK message to the server.
    #[serde(default = "default_true")]
    pub forward_raw: bool,
}

fn default_start_directory() -> String {
    "~".into()
}

fn default_true() -> bool {
    true
}

/// Where the daemon keeps its files.
#[derive(clap::Args, Clone, Debug)]
pub struct DirArgs {
    /// Directory for daemon.toml and the daemon's state.
    #[arg(long = "dir", env = "BLUE_DAEMON_DIR", global = true)]
    pub dir: Option<PathBuf>,
}

impl DirArgs {
    pub fn dir(&self) -> anyhow::Result<PathBuf> {
        match &self.dir {
            Some(dir) => Ok(dir.clone()),
            None => Ok(dirs::config_dir()
                .context("no per-user config directory on this OS")?
                .join("blue")),
        }
    }
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join("daemon.toml")
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("daemon-state.sqlite")
}

/// The machine token: the OS keyring, or `machine-token` in `dir`. With `BLUE_DAEMON_DIR` set,
/// only the file is used, so separate daemon directories never share a keyring entry.
pub fn token_store(dir: &Path, explicit_dir: bool, machine_id: &str) -> CredentialStore {
    let file = dir.join("machine-token");
    if explicit_dir {
        CredentialStore::file_only(file)
    } else {
        CredentialStore::new("blue-daemon", machine_id, file)
    }
}

impl DaemonConfig {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = config_path(dir);
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "read {} (pair this machine first: blue-daemon pair --server URL --code CODE)",
                path.display()
            )
        })?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let path = config_path(dir);
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))
    }

    pub fn claude_path(&self) -> Option<PathBuf> {
        non_empty(&self.claude_path)
    }

    pub fn bridge_path(&self) -> Option<PathBuf> {
        non_empty(&self.bridge_path)
    }

    pub fn start_directory(&self) -> anyhow::Result<PathBuf> {
        let dir = self.start_directory.trim();
        let home = || std::env::home_dir().context("no home directory");
        let path = if dir.is_empty() || dir == "~" {
            home()?
        } else if let Some(rest) = dir.strip_prefix("~/").or_else(|| dir.strip_prefix("~\\")) {
            home()?.join(rest)
        } else {
            PathBuf::from(dir)
        };
        Ok(std::path::absolute(path)?)
    }
}

fn non_empty(path: &str) -> Option<PathBuf> {
    (!path.trim().is_empty()).then(|| PathBuf::from(path.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            config_path(dir.path()),
            "server_url = \"http://localhost:50051\"\nmachine_name = \"desktop\"\nmachine_id = \"m1\"\n",
        )
        .unwrap();
        let config = DaemonConfig::load(dir.path()).unwrap();
        assert!(config.forward_raw);
        assert_eq!(config.claude_path(), None);
        assert_eq!(
            config.start_directory().unwrap(),
            std::env::home_dir().unwrap()
        );
        config.save(dir.path()).unwrap();
        assert_eq!(
            DaemonConfig::load(dir.path()).unwrap().machine_name,
            "desktop"
        );
    }
}
