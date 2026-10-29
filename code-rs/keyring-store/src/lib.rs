use keyring::Entry;
use keyring::Error as KeyringError;
use sha2::Digest;
use sha2::Sha256;
use std::error::Error;
use std::fmt;
use std::fmt::Debug;
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
use std::io::Write;
use std::path::Path;
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
use std::process::Command;
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
use std::process::Output;
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
use std::process::Stdio;
use std::sync::Arc;
use tracing::trace;

#[cfg(target_os = "macos")]
mod macos;

#[derive(Debug)]
pub enum CredentialStoreError {
    Other(KeyringError),
}

impl CredentialStoreError {
    pub fn new(error: KeyringError) -> Self {
        Self::Other(error)
    }

    pub fn message(&self) -> String {
        match self {
            Self::Other(error) => error.to_string(),
        }
    }

    pub fn into_error(self) -> KeyringError {
        match self {
            Self::Other(error) => error,
        }
    }
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

impl Error for CredentialStoreError {}

/// Return a deterministic keyring account key for a given `CODE_HOME` path.
///
/// The keyring API splits storage into `{service, account}`. We want the
/// account key to remain stable per `CODE_HOME`, while avoiding leaking the
/// full path into OS credential UIs.
pub fn store_key_for_code_home(prefix: &str, code_home: &Path) -> String {
    let canonical = code_home
        .canonicalize()
        .unwrap_or_else(|_| code_home.to_path_buf());
    let path_str = canonical.to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(path_str.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    let truncated = hex.get(..16).unwrap_or(&hex);
    format!("{prefix}|{truncated}")
}

/// Shared credential store abstraction for keyring-backed implementations.
pub trait KeyringStore: Debug + Send + Sync {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError>;
    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError>;
    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError>;
}

/// Return the best available [`KeyringStore`] for the current platform.
///
/// On macOS, this returns a store backed by the Data Protection Keychain,
/// which integrates with Touch ID and avoids the legacy "Allow/Deny/Always
/// Allow" Keychain Access dialogs.  On all other platforms, it returns the
/// default `keyring`-crate-backed store.
pub fn best_keyring_store() -> Arc<dyn KeyringStore> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::DataProtectionKeyringStore)
    }
    #[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
    {
        Arc::new(LinuxKeyctlStore)
    }
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", feature = "cosmopolitan-ape")
    )))]
    {
        Arc::new(DefaultKeyringStore)
    }
}

/// Kernel-keyring storage for the portable Cosmopolitan build on Linux.
///
/// The APE cannot rely on a desktop Secret Service being activatable, and its
/// own libc does not implement the keyring syscalls used by the `keyring`
/// crate's Linux backend. Invoking the host-native `keyctl` helper keeps
/// credentials out of files while making them available to other Code
/// processes in the same login session.
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
#[derive(Debug)]
pub struct LinuxKeyctlStore;

#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
impl LinuxKeyctlStore {
    fn description(service: &str, account: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(service.as_bytes());
        hasher.update([0]);
        hasher.update(account.as_bytes());
        format!("code-keyring-v1:{:x}", hasher.finalize())
    }

    fn platform_error(message: impl Into<String>) -> CredentialStoreError {
        CredentialStoreError::new(KeyringError::PlatformFailure(Box::new(
            std::io::Error::other(message.into()),
        )))
    }

    fn run(args: &[&str], input: Option<&[u8]>) -> Result<Output, CredentialStoreError> {
        let mut command = Command::new("keyctl");
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Self::platform_error(format!("failed to run keyctl: {error}")))?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or_else(|| Self::platform_error("failed to open keyctl standard input"))?
                .write_all(input)
                .map_err(|error| {
                    Self::platform_error(format!("failed to write keyctl standard input: {error}"))
                })?;
        }
        child
            .wait_with_output()
            .map_err(|error| Self::platform_error(format!("failed to wait for keyctl: {error}")))
    }

    fn command_error(action: &str, output: &Output) -> CredentialStoreError {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let message = if detail.is_empty() {
            format!("keyctl {action} failed with status {}", output.status)
        } else {
            format!(
                "keyctl {action} failed with status {}: {detail}",
                output.status
            )
        };
        Self::platform_error(message)
    }

    fn search(description: &str) -> Result<Option<String>, CredentialStoreError> {
        let output = Self::run(&["search", "@u", "user", description], None)?;
        if output.status.success() {
            let key_id = std::str::from_utf8(&output.stdout)
                .map_err(|error| {
                    Self::platform_error(format!("keyctl returned an invalid key ID: {error}"))
                })?
                .trim();
            key_id.parse::<i64>().map_err(|error| {
                Self::platform_error(format!("keyctl returned an invalid key ID: {error}"))
            })?;
            return Ok(Some(key_id.to_string()));
        }

        let keyring = Self::run(&["list", "@u"], None)?;
        if keyring.status.success() {
            Ok(None)
        } else {
            Err(Self::command_error("search", &output))
        }
    }
}

#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
impl KeyringStore for LinuxKeyctlStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        let description = Self::description(service, account);
        let Some(key_id) = Self::search(&description)? else {
            return Ok(None);
        };
        let output = Self::run(&["pipe", &key_id], None)?;
        if !output.status.success() {
            return Err(Self::command_error("pipe", &output));
        }
        String::from_utf8(output.stdout).map(Some).map_err(|error| {
            Self::platform_error(format!("keyctl returned invalid UTF-8: {error}"))
        })
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        let description = Self::description(service, account);
        let output = Self::run(
            &["padd", "user", &description, "@u"],
            Some(value.as_bytes()),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::command_error("padd", &output))
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        let description = Self::description(service, account);
        let Some(key_id) = Self::search(&description)? else {
            return Ok(false);
        };
        let output = Self::run(&["unlink", &key_id, "@u"], None)?;
        if output.status.success() {
            Ok(true)
        } else {
            Err(Self::command_error("unlink", &output))
        }
    }
}

#[derive(Debug)]
pub struct DefaultKeyringStore;

impl KeyringStore for DefaultKeyringStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        trace!("keyring.load start, service={service}, account={account}");
        let entry = Entry::new(service, account).map_err(CredentialStoreError::new)?;
        match entry.get_password() {
            Ok(password) => {
                trace!("keyring.load success, service={service}, account={account}");
                Ok(Some(password))
            }
            Err(keyring::Error::NoEntry) => {
                trace!("keyring.load no entry, service={service}, account={account}");
                Ok(None)
            }
            Err(error) => {
                trace!("keyring.load error, service={service}, account={account}, error={error}");
                Err(CredentialStoreError::new(error))
            }
        }
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        trace!(
            "keyring.save start, service={service}, account={account}, value_len={}",
            value.len()
        );
        let entry = Entry::new(service, account).map_err(CredentialStoreError::new)?;
        match entry.set_password(value) {
            Ok(()) => {
                trace!("keyring.save success, service={service}, account={account}");
                Ok(())
            }
            Err(error) => {
                trace!("keyring.save error, service={service}, account={account}, error={error}");
                Err(CredentialStoreError::new(error))
            }
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        trace!("keyring.delete start, service={service}, account={account}");
        let entry = Entry::new(service, account).map_err(CredentialStoreError::new)?;
        match entry.delete_credential() {
            Ok(()) => {
                trace!("keyring.delete success, service={service}, account={account}");
                Ok(true)
            }
            Err(keyring::Error::NoEntry) => {
                trace!("keyring.delete no entry, service={service}, account={account}");
                Ok(false)
            }
            Err(error) => {
                trace!("keyring.delete error, service={service}, account={account}, error={error}");
                Err(CredentialStoreError::new(error))
            }
        }
    }
}

pub mod tests {
    use super::CredentialStoreError;
    use super::KeyringStore;
    use keyring::Error as KeyringError;
    use keyring::credential::CredentialApi as _;
    use keyring::mock::MockCredential;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::PoisonError;

    #[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
    #[test]
    fn cosmopolitan_ape_uses_the_host_keyctl_backend() {
        assert_eq!(
            format!("{:?}", super::best_keyring_store()),
            "LinuxKeyctlStore"
        );
    }

    #[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
    #[test]
    fn cosmopolitan_ape_keyctl_round_trip() {
        let store = super::best_keyring_store();
        let account = format!(
            "code-keyring-store-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let result = (|| {
            store.save("Code Test", &account, "first-value")?;
            store.save("Code Test", &account, "replacement-value")?;
            assert_eq!(
                store.load("Code Test", &account)?.as_deref(),
                Some("replacement-value")
            );
            assert!(store.delete("Code Test", &account)?);
            assert_eq!(store.load("Code Test", &account)?, None);
            Ok::<_, CredentialStoreError>(())
        })();
        let _ = store.delete("Code Test", &account);
        result.unwrap();
    }

    #[derive(Default, Clone, Debug)]
    pub struct MockKeyringStore {
        credentials: Arc<Mutex<HashMap<String, Arc<MockCredential>>>>,
    }

    impl MockKeyringStore {
        pub fn credential(&self, account: &str) -> Arc<MockCredential> {
            let mut guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .entry(account.to_owned())
                .or_insert_with(|| Arc::new(MockCredential::default()))
                .clone()
        }

        pub fn saved_value(&self, account: &str) -> Option<String> {
            let credential = {
                let guard = self
                    .credentials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.get(account).cloned()
            }?;
            credential.get_password().ok()
        }

        pub fn set_error(&self, account: &str, error: KeyringError) {
            let credential = self.credential(account);
            credential.set_error(error);
        }

        pub fn contains(&self, account: &str) -> bool {
            let guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.contains_key(account)
        }
    }

    impl KeyringStore for MockKeyringStore {
        fn load(
            &self,
            _service: &str,
            account: &str,
        ) -> Result<Option<String>, CredentialStoreError> {
            let credential = {
                let guard = self
                    .credentials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.get(account).cloned()
            };

            let Some(credential) = credential else {
                return Ok(None);
            };

            match credential.get_password() {
                Ok(password) => Ok(Some(password)),
                Err(KeyringError::NoEntry) => Ok(None),
                Err(error) => Err(CredentialStoreError::new(error)),
            }
        }

        fn save(
            &self,
            _service: &str,
            account: &str,
            value: &str,
        ) -> Result<(), CredentialStoreError> {
            let credential = self.credential(account);
            credential
                .set_password(value)
                .map_err(CredentialStoreError::new)
        }

        fn delete(&self, _service: &str, account: &str) -> Result<bool, CredentialStoreError> {
            let credential = {
                let guard = self
                    .credentials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.get(account).cloned()
            };

            let Some(credential) = credential else {
                return Ok(false);
            };

            let removed = match credential.delete_credential() {
                Ok(()) => Ok(true),
                Err(KeyringError::NoEntry) => Ok(false),
                Err(error) => Err(CredentialStoreError::new(error)),
            }?;

            let mut guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.remove(account);
            Ok(removed)
        }
    }
}
