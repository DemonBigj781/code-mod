use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use age::decrypt;
use age::encrypt;
use age::scrypt::Identity as ScryptIdentity;
use age::scrypt::Recipient as ScryptRecipient;
use age::secrecy::ExposeSecret;
use age::secrecy::SecretString;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use fs2::FileExt;
use keyring::Error as KeyringError;
use rand::TryRngCore;
use rand::rngs::OsRng;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tracing::trace;
use zeroize::Zeroize;

use super::CredentialStoreError;
use super::KeyringStore;

const STORE_VERSION: u8 = 1;
const STORE_DIR: &str = "keyring";
const VAULT_FILE: &str = "credentials.age";
const PASSPHRASE_FILE: &str = "passphrase";
const LOCK_FILE: &str = "credentials.lock";

#[derive(Debug, Serialize, Deserialize)]
struct Vault {
    version: u8,
    entries: BTreeMap<String, String>,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

/// Code-owned encrypted credential storage for Linux.
///
/// The KDE/Secret Service backend is retained only as a read-through migration
/// source. New writes go exclusively to the encrypted vault under `CODE_HOME`,
/// so Code no longer requires a running desktop wallet after each legacy entry
/// has been read once.
pub struct EmbeddedKeyringStore {
    code_home: PathBuf,
    legacy: Option<Arc<dyn KeyringStore>>,
    process_lock: Mutex<()>,
}

impl std::fmt::Debug for EmbeddedKeyringStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedKeyringStore")
            .field("code_home", &self.code_home)
            .field("legacy_migration", &self.legacy.is_some())
            .finish_non_exhaustive()
    }
}

impl EmbeddedKeyringStore {
    pub fn new(code_home: PathBuf, legacy: Option<Arc<dyn KeyringStore>>) -> Self {
        let code_home = code_home.canonicalize().unwrap_or(code_home);
        Self {
            code_home,
            legacy,
            process_lock: Mutex::new(()),
        }
    }

    fn store_dir(&self) -> PathBuf {
        self.code_home.join(STORE_DIR)
    }

    pub(crate) fn vault_path(&self) -> PathBuf {
        self.store_dir().join(VAULT_FILE)
    }

    fn passphrase_path(&self) -> PathBuf {
        self.store_dir().join(PASSPHRASE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.store_dir().join(LOCK_FILE)
    }

    fn entry_key(service: &str, account: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(service.as_bytes());
        hasher.update([0]);
        hasher.update(account.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn ensure_store_dir(&self) -> Result<(), CredentialStoreError> {
        let path = self.store_dir();
        if path.exists() {
            if !path.is_dir() || path.is_symlink() {
                return Err(platform_error(format!(
                    "embedded keyring path is not a safe directory: {}",
                    path.display()
                )));
            }
            return Ok(());
        }

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|error| {
            platform_error(format!(
                "failed to create embedded keyring directory {}: {error}",
                path.display()
            ))
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, CredentialStoreError>,
    ) -> Result<T, CredentialStoreError> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| platform_error("embedded keyring process lock was poisoned"))?;
        self.ensure_store_dir()?;
        let lock_file = open_private_file(&self.lock_path(), true, true)?;
        lock_file.lock_exclusive().map_err(|error| {
            platform_error(format!("failed to lock embedded keyring: {error}"))
        })?;
        let result = operation();
        let unlock_result = FileExt::unlock(&lock_file).map_err(|error| {
            platform_error(format!("failed to unlock embedded keyring: {error}"))
        });
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn load_local(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        self.with_lock(|| {
            let vault = self.read_vault()?;
            Ok(vault.entries.get(&Self::entry_key(service, account)).cloned())
        })
    }

    fn save_local(
        &self,
        service: &str,
        account: &str,
        value: &str,
    ) -> Result<(), CredentialStoreError> {
        self.with_lock(|| {
            let mut vault = self.read_vault()?;
            vault
                .entries
                .insert(Self::entry_key(service, account), value.to_owned());
            self.write_vault(&vault)
        })
    }

    fn delete_local(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        self.with_lock(|| {
            let mut vault = self.read_vault()?;
            let removed = vault
                .entries
                .remove(&Self::entry_key(service, account))
                .is_some();
            if removed {
                self.write_vault(&vault)?;
            }
            Ok(removed)
        })
    }

    fn read_vault(&self) -> Result<Vault, CredentialStoreError> {
        let path = self.vault_path();
        if !path.exists() {
            return Ok(Vault::default());
        }
        let mut file = open_private_file(&path, false, false)?;
        let mut ciphertext = Vec::new();
        file.read_to_end(&mut ciphertext).map_err(|error| {
            platform_error(format!(
                "failed to read embedded keyring vault {}: {error}",
                path.display()
            ))
        })?;
        let passphrase = self.read_passphrase()?.ok_or_else(|| {
            platform_error(format!(
                "embedded keyring passphrase is missing for {}",
                path.display()
            ))
        })?;
        let identity = ScryptIdentity::new(passphrase);
        let mut plaintext = decrypt(&identity, &ciphertext).map_err(|error| {
            platform_error(format!("failed to decrypt embedded keyring: {error}"))
        })?;
        let vault: Vault = serde_json::from_slice(&plaintext).map_err(|error| {
            platform_error(format!("failed to parse embedded keyring vault: {error}"))
        })?;
        plaintext.zeroize();
        if vault.version != STORE_VERSION {
            return Err(platform_error(format!(
                "unsupported embedded keyring version {}",
                vault.version
            )));
        }
        Ok(vault)
    }

    fn write_vault(&self, vault: &Vault) -> Result<(), CredentialStoreError> {
        let passphrase = self.read_or_create_passphrase()?;
        let mut plaintext = serde_json::to_vec(vault).map_err(|error| {
            platform_error(format!("failed to serialize embedded keyring vault: {error}"))
        })?;
        let recipient = ScryptRecipient::new(passphrase);
        let ciphertext = encrypt(&recipient, &plaintext).map_err(|error| {
            platform_error(format!("failed to encrypt embedded keyring vault: {error}"))
        })?;
        plaintext.zeroize();
        write_private_file_atomically(&self.vault_path(), &ciphertext)
    }

    fn read_or_create_passphrase(&self) -> Result<SecretString, CredentialStoreError> {
        if let Some(passphrase) = self.read_passphrase()? {
            return Ok(passphrase);
        }

        let mut random = [0_u8; 32];
        let mut rng = OsRng;
        rng.try_fill_bytes(&mut random).map_err(|error| {
            platform_error(format!("failed to generate embedded keyring key: {error}"))
        })?;
        let encoded = BASE64_STANDARD.encode(random);
        random.zeroize();
        let passphrase = SecretString::from(encoded);
        let mut file = open_private_file(&self.passphrase_path(), true, true)?;
        file.write_all(passphrase.expose_secret().as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                platform_error(format!("failed to persist embedded keyring key: {error}"))
            })?;
        Ok(passphrase)
    }

    fn read_passphrase(&self) -> Result<Option<SecretString>, CredentialStoreError> {
        let path = self.passphrase_path();
        if !path.exists() {
            return Ok(None);
        }
        let mut file = open_private_file(&path, false, false)?;
        let mut value = String::new();
        file.read_to_string(&mut value).map_err(|error| {
            platform_error(format!("failed to read embedded keyring key: {error}"))
        })?;
        let trimmed = value.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Err(platform_error("embedded keyring key is empty"));
        }
        Ok(Some(SecretString::from(trimmed.to_owned())))
    }
}

impl KeyringStore for EmbeddedKeyringStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        if let Some(value) = self.load_local(service, account)? {
            return Ok(Some(value));
        }

        let Some(legacy) = self.legacy.as_ref() else {
            return Ok(None);
        };
        match legacy.load(service, account) {
            Ok(Some(value)) => {
                self.save_local(service, account, &value)?;
                trace!("migrated legacy credential into embedded keyring, service={service}, account={account}");
                Ok(Some(value))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                trace!("legacy keyring unavailable during optional migration: {error}");
                Ok(None)
            }
        }
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        self.save_local(service, account, value)
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        let removed_local = self.delete_local(service, account)?;
        let removed_legacy = self
            .legacy
            .as_ref()
            .and_then(|legacy| match legacy.delete(service, account) {
                Ok(removed) => Some(removed),
                Err(error) => {
                    trace!("legacy keyring unavailable during optional cleanup: {error}");
                    None
                }
            })
            .unwrap_or(false);
        Ok(removed_local || removed_legacy)
    }
}

fn open_private_file(
    path: &Path,
    create: bool,
    write: bool,
) -> Result<File, CredentialStoreError> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| {
        platform_error(format!(
            "failed to open embedded keyring file {}: {error}",
            path.display()
        ))
    })
}

fn write_private_file_atomically(path: &Path, contents: &[u8]) -> Result<(), CredentialStoreError> {
    let parent = path.parent().ok_or_else(|| {
        platform_error(format!("embedded keyring path has no parent: {}", path.display()))
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temp = parent.join(format!(".{VAULT_FILE}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = open_private_file(&temp, true, true)?;
        file.write_all(contents)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                platform_error(format!("failed to write embedded keyring vault: {error}"))
            })?;
        fs::rename(&temp, path).map_err(|error| {
            platform_error(format!(
                "failed to replace embedded keyring vault {}: {error}",
                path.display()
            ))
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn platform_error(message: impl Into<String>) -> CredentialStoreError {
    CredentialStoreError::new(KeyringError::PlatformFailure(Box::new(
        std::io::Error::other(message.into()),
    )))
}
