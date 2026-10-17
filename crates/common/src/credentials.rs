//! A secret token kept in the OS keyring (Windows Credential Manager, macOS Keychain), falling
//! back to a file only the current user can read. Linux always uses the file: the kernel keyring
//! doesn't survive reboots and the Secret Service needs a desktop session, which servers lack.

use std::path::PathBuf;

use anyhow::Context;

pub struct CredentialStore {
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    service: String,
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    account: String,
    file: PathBuf,
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    use_keyring: bool,
}

impl CredentialStore {
    /// `service`/`account` name the keyring entry; `file` is the fallback location.
    pub fn new(service: &str, account: &str, file: PathBuf) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
            file,
            use_keyring: true,
        }
    }

    /// Only the file, never the keyring; for tests and throwaway local setups.
    pub fn file_only(file: PathBuf) -> Self {
        Self {
            service: String::new(),
            account: String::new(),
            file,
            use_keyring: false,
        }
    }

    pub fn get(&self) -> anyhow::Result<Option<String>> {
        if let Some(token) = self.keyring_get() {
            return Ok(Some(token));
        }
        match std::fs::read_to_string(&self.file) {
            Ok(token) => Ok(Some(token.trim().to_owned())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("read {}", self.file.display())),
        }
    }

    pub fn set(&self, token: &str) -> anyhow::Result<()> {
        if self.keyring_set(token) {
            // Don't leave an older copy behind in the fallback file.
            let _ = std::fs::remove_file(&self.file);
            return Ok(());
        }
        write_private(&self.file, token)
    }

    pub fn delete(&self) -> anyhow::Result<()> {
        self.keyring_delete();
        match std::fs::remove_file(&self.file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(err).with_context(|| format!("remove {}", self.file.display()))
            }
            _ => Ok(()),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn entry(&self) -> Option<keyring::Entry> {
        if !self.use_keyring {
            return None;
        }
        keyring::Entry::new(&self.service, &self.account).ok()
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn keyring_get(&self) -> Option<String> {
        self.entry()?.get_password().ok()
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn keyring_set(&self, token: &str) -> bool {
        self.entry()
            .is_some_and(|entry| entry.set_password(token).is_ok())
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn keyring_delete(&self) {
        if let Some(entry) = self.entry() {
            let _ = entry.delete_credential();
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn keyring_get(&self) -> Option<String> {
        None
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn keyring_set(&self, _token: &str) -> bool {
        false
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn keyring_delete(&self) {}
}

/// Writes `contents` to a file readable only by the current user (mode 0600 on Unix; files under
/// the user's profile are already private on Windows).
fn write_private(path: &PathBuf, contents: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    std::io::Write::write_all(&mut file, contents.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}
