use keyring::Entry;
use keyring::Error as KeyringError;
use sha2::Digest;
use sha2::Sha256;
use std::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::path::Path;
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
        Arc::new(LinuxKernelKeyringStore)
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
/// compile-time Linux target would select that D-Bus backend even when the same
/// artifact is running on another host. The kernel keyring keeps credentials
/// out of files and makes them available to other Code processes in the same
/// login session.
#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
#[derive(Debug)]
pub struct LinuxKernelKeyringStore;

#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
impl LinuxKernelKeyringStore {
    fn entry(service: &str, account: &str) -> Result<Entry, CredentialStoreError> {
        let credential =
            keyring::keyutils::KeyutilsCredential::new_with_target(None, service, account)
                .map_err(CredentialStoreError::new)?;
        Ok(Entry::new_with_credential(Box::new(credential)))
    }
}

#[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
impl KeyringStore for LinuxKernelKeyringStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        let entry = Self::entry(service, account)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialStoreError::new(error)),
        }
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        Self::entry(service, account)?
            .set_password(value)
            .map_err(CredentialStoreError::new)
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(CredentialStoreError::new(error)),
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
    fn cosmopolitan_ape_uses_the_linux_kernel_keyring() {
        assert_eq!(
            format!("{:?}", super::best_keyring_store()),
            "LinuxKernelKeyringStore"
        );
    }

    #[cfg(all(target_os = "linux", feature = "cosmopolitan-ape"))]
    #[test]
    fn cosmopolitan_ape_kernel_keyring_round_trip() {
        let store = super::best_keyring_store();
        let account = format!(
            "code-keyring-store-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let value = "temporary-test-credential";

        store.save("Code Test", &account, value).unwrap();
        assert_eq!(
            store.load("Code Test", &account).unwrap().as_deref(),
            Some(value)
        );
        assert!(store.delete("Code Test", &account).unwrap());
        assert_eq!(store.load("Code Test", &account).unwrap(), None);
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
