use keyring::Entry;
use keyring::Error as KeyringError;
use sha2::Digest;
use sha2::Sha256;
#[cfg(all(
    target_arch = "x86_64",
    target_env = "musl",
    target_os = "linux",
    feature = "cosmopolitan-ape"
))]
use std::arch::asm;
use std::error::Error;
#[cfg(all(
    target_arch = "x86_64",
    target_env = "musl",
    target_os = "linux",
    feature = "cosmopolitan-ape"
))]
use std::ffi::CString;
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
    #[cfg(all(
        target_arch = "x86_64",
        target_env = "musl",
        target_os = "linux",
        feature = "cosmopolitan-ape"
    ))]
    {
        Arc::new(LinuxApeKernelKeyringStore)
    }
    #[cfg(not(any(
        target_os = "macos",
        all(
            target_arch = "x86_64",
            target_env = "musl",
            target_os = "linux",
            feature = "cosmopolitan-ape"
        )
    )))]
    {
        Arc::new(DefaultKeyringStore)
    }
}

/// Kernel-keyring storage for the portable Cosmopolitan build on Linux.
///
/// The APE cannot rely on a desktop Secret Service being activatable, and its
/// own libc does not implement the keyring syscalls used by the `keyring`
/// crate's Linux backend. Linux-gated raw syscalls keep credentials out of
/// files while making them available to other Code processes owned by the same
/// user.
#[cfg(all(
    target_arch = "x86_64",
    target_env = "musl",
    target_os = "linux",
    feature = "cosmopolitan-ape"
))]
#[derive(Debug)]
pub struct LinuxApeKernelKeyringStore;

#[cfg(all(
    target_arch = "x86_64",
    target_env = "musl",
    target_os = "linux",
    feature = "cosmopolitan-ape"
))]
impl LinuxApeKernelKeyringStore {
    const HOST_LINUX: i32 = 1;
    const SYS_ADD_KEY: isize = 248;
    const SYS_KEYCTL: isize = 250;
    const KEY_SPEC_USER_KEYRING: isize = -4;
    const KEYCTL_SET_PERM: isize = 5;
    const KEYCTL_UNLINK: isize = 9;
    const KEYCTL_SEARCH: isize = 10;
    const KEYCTL_READ: isize = 11;
    const OWNER_PERMISSIONS: isize = 0x3f3f0000;
    const ENOKEY: isize = 126;

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

    fn host_is_linux() -> bool {
        unsafe extern "C" {
            static __hostos: i32;
        }
        // SAFETY: `__hostos` is a read-only Cosmopolitan runtime symbol that is
        // present in every APE linked by the pinned toolchain.
        unsafe { __hostos & Self::HOST_LINUX != 0 }
    }

    fn raw_syscall5(
        number: isize,
        arg1: isize,
        arg2: isize,
        arg3: isize,
        arg4: isize,
        arg5: isize,
    ) -> isize {
        let result: isize;
        // SAFETY: This is the x86-64 Linux syscall ABI. Every call is gated by
        // `host_is_linux`, and all pointer/length pairs below remain valid for
        // the duration of the instruction.
        unsafe {
            asm!(
                "syscall",
                inlateout("rax") number => result,
                in("rdi") arg1,
                in("rsi") arg2,
                in("rdx") arg3,
                in("r10") arg4,
                in("r8") arg5,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        result
    }

    fn checked(action: &str, result: isize) -> Result<isize, CredentialStoreError> {
        if result < 0 && result > -4096 {
            let error = std::io::Error::from_raw_os_error((-result) as i32);
            Err(Self::platform_error(format!(
                "Linux keyring {action} failed: {error}"
            )))
        } else {
            Ok(result)
        }
    }

    fn ensure_linux() -> Result<(), CredentialStoreError> {
        if Self::host_is_linux() {
            Ok(())
        } else {
            Err(Self::platform_error(
                "Linux kernel credential storage is unavailable on this APE host",
            ))
        }
    }

    fn search(description: &CString) -> Result<Option<isize>, CredentialStoreError> {
        let result = Self::raw_syscall5(
            Self::SYS_KEYCTL,
            Self::KEYCTL_SEARCH,
            Self::KEY_SPEC_USER_KEYRING,
            c"user".as_ptr() as isize,
            description.as_ptr() as isize,
            0,
        );
        if result == -Self::ENOKEY {
            Ok(None)
        } else {
            Self::checked("search", result).map(Some)
        }
    }

    fn remove_key(key_id: isize) -> Result<(), CredentialStoreError> {
        Self::checked(
            "unlink",
            Self::raw_syscall5(
                Self::SYS_KEYCTL,
                Self::KEYCTL_UNLINK,
                key_id,
                Self::KEY_SPEC_USER_KEYRING,
                0,
                0,
            ),
        )?;
        Ok(())
    }
}

#[cfg(all(
    target_arch = "x86_64",
    target_env = "musl",
    target_os = "linux",
    feature = "cosmopolitan-ape"
))]
impl KeyringStore for LinuxApeKernelKeyringStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        Self::ensure_linux()?;
        let description = CString::new(Self::description(service, account))
            .map_err(|error| Self::platform_error(format!("invalid key description: {error}")))?;
        let Some(key_id) = Self::search(&description)? else {
            return Ok(None);
        };
        let size = Self::checked(
            "read size",
            Self::raw_syscall5(Self::SYS_KEYCTL, Self::KEYCTL_READ, key_id, 0, 0, 0),
        )? as usize;
        let mut value = vec![0_u8; size];
        if size != 0 {
            let read = Self::checked(
                "read",
                Self::raw_syscall5(
                    Self::SYS_KEYCTL,
                    Self::KEYCTL_READ,
                    key_id,
                    value.as_mut_ptr() as isize,
                    value.len() as isize,
                    0,
                ),
            )? as usize;
            if read > value.len() {
                return Err(Self::platform_error(
                    "Linux keyring payload changed while it was being read",
                ));
            }
            value.truncate(read);
        }
        String::from_utf8(value).map(Some).map_err(|error| {
            Self::platform_error(format!("Linux keyring returned invalid UTF-8: {error}"))
        })
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        Self::ensure_linux()?;
        let description = CString::new(Self::description(service, account))
            .map_err(|error| Self::platform_error(format!("invalid key description: {error}")))?;
        let key_id = Self::checked(
            "save",
            Self::raw_syscall5(
                Self::SYS_ADD_KEY,
                c"user".as_ptr() as isize,
                description.as_ptr() as isize,
                value.as_ptr() as isize,
                value.len() as isize,
                Self::KEY_SPEC_USER_KEYRING,
            ),
        )?;
        if let Err(error) = Self::checked(
            "set permissions",
            Self::raw_syscall5(
                Self::SYS_KEYCTL,
                Self::KEYCTL_SET_PERM,
                key_id,
                Self::OWNER_PERMISSIONS,
                0,
                0,
            ),
        ) {
            let _ = Self::remove_key(key_id);
            return Err(error);
        }
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        Self::ensure_linux()?;
        let description = CString::new(Self::description(service, account))
            .map_err(|error| Self::platform_error(format!("invalid key description: {error}")))?;
        let Some(key_id) = Self::search(&description)? else {
            return Ok(false);
        };
        Self::remove_key(key_id)?;
        Ok(true)
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

    #[cfg(all(
        target_arch = "x86_64",
        target_env = "musl",
        target_os = "linux",
        feature = "cosmopolitan-ape"
    ))]
    #[test]
    fn cosmopolitan_ape_uses_the_raw_linux_keyring_backend() {
        assert_eq!(
            format!("{:?}", super::best_keyring_store()),
            "LinuxApeKernelKeyringStore"
        );
    }

    #[cfg(all(
        target_arch = "x86_64",
        target_env = "musl",
        target_os = "linux",
        feature = "cosmopolitan-ape"
    ))]
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
        let result = (|| {
            store.save("Code Test", &account, "first-value")?;
            store.save("Code Test", &account, "replacement-value")?;
            assert_eq!(
                store.load("Code Test", &account)?.as_deref(),
                Some("replacement-value")
            );
            let description = super::LinuxApeKernelKeyringStore::description("Code Test", &account);
            let search = std::process::Command::new("keyctl")
                .args(["search", "@u", "user", &description])
                .output()
                .expect("keyctl should be installed by the APE build job");
            assert!(
                search.status.success(),
                "{}",
                String::from_utf8_lossy(&search.stderr)
            );
            let key_id = String::from_utf8(search.stdout).unwrap();
            let read = std::process::Command::new("keyctl")
                .args(["pipe", key_id.trim()])
                .output()
                .unwrap();
            assert!(
                read.status.success(),
                "{}",
                String::from_utf8_lossy(&read.stderr)
            );
            assert_eq!(read.stdout, b"replacement-value");
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
