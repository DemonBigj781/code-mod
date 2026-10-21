//! Operator-managed SSH connections. This module never opens a network connection.

use anyhow::{Context, Result, bail, ensure};
use code_keyring_store::{KeyringStore, best_keyring_store, store_key_for_code_home};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const FILENAME: &str = "ssh-connections.json";
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
const CREDENTIAL_SERVICE: &str = "Code SSH";

pub const HELP: &str = "/ssh list | show <name> | add <user@host[,user@host...]> <pwd|key> <name> | update <name> <endpoints> | rename <name> <new-name> | auth <name> <pwd|key> | remove <name>";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum AuthMethod {
    #[serde(rename = "pwd")]
    Password,
    #[serde(rename = "key")]
    Key,
}

impl AuthMethod {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "pwd" => Ok(Self::Password),
            "key" => Ok(Self::Key),
            _ => bail!("authentication must be pwd or key; never put credentials in the command"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "pwd",
            Self::Key => "key",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub user: String,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        ensure!(
            !input.is_empty() && !input.chars().any(char::is_whitespace),
            "invalid SSH endpoint"
        );
        let url = if input.starts_with("ssh://") {
            input.to_owned()
        } else {
            format!("ssh://{input}")
        };
        let parsed = url::Url::parse(&url).map_err(|_| anyhow::anyhow!("invalid SSH endpoint"))?;
        ensure!(
            parsed.scheme() == "ssh" && parsed.password().is_none(),
            "use an SSH endpoint without inline credentials"
        );
        ensure!(
            parsed.query().is_none()
                && parsed.fragment().is_none()
                && matches!(parsed.path(), "" | "/"),
            "SSH registration accepts an endpoint, not a path, query, or fragment"
        );
        let user = parsed.username().to_owned();
        let host = parsed
            .host_str()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let endpoint = Self {
            user,
            host,
            port: parsed.port().unwrap_or(22),
        };
        endpoint.validate()?;
        Ok(endpoint)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.user.is_empty()
                && self.user.len() <= 256
                && !self.user.starts_with('-')
                && self
                    .user
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
            "SSH endpoint requires a valid username"
        );
        ensure!(self.port != 0, "SSH port must be greater than zero");
        let ip = self.host.parse::<IpAddr>();
        let hostname = !self.host.is_empty()
            && self.host.len() <= 253
            && self.host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
        ensure!(ip.is_ok() || hostname, "invalid SSH host");
        if self.host.chars().all(|c| c.is_ascii_digit() || c == '.') {
            ensure!(
                self.host.parse::<Ipv4Addr>().is_ok(),
                "invalid IPv4 endpoint"
            );
        }
        Ok(())
    }

    pub fn is_deferred(&self) -> bool {
        let ipv4 = match self.host.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => Some(ip),
            Ok(IpAddr::V6(ip)) => ip.to_ipv4_mapped(),
            Err(_) => None,
        };
        ipv4.is_some_and(|ip| {
            let octets = ip.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        })
    }

    pub fn display(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == 22 {
            format!("{}@{host}", self.user)
        } else {
            format!("{}@{host}:{}", self.user, self.port)
        }
    }
}

pub fn parse_endpoints(input: &str) -> Result<Vec<Endpoint>> {
    let mut seen = HashSet::new();
    let endpoints = input
        .split(',')
        .map(Endpoint::parse)
        .collect::<Result<Vec<_>>>()?;
    for endpoint in &endpoints {
        ensure!(seen.insert(endpoint.display()), "duplicate SSH endpoint");
    }
    Ok(endpoints)
}

pub fn is_local_alias(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "self" | "127.0.0.1" | "0.0.0.0"
    )
}

pub fn resolve_key_path(raw: &str, cwd: &Path) -> Result<Option<PathBuf>> {
    if raw.is_empty() {
        return Ok(None);
    }
    if let Some(suffix) = raw.strip_prefix("~/") {
        return Ok(Some(
            dirs::home_dir()
                .context("cannot resolve the home directory")?
                .join(suffix),
        ));
    }
    let path = PathBuf::from(raw);
    Ok(Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }))
}

fn name_key(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty()
            && name == name.trim()
            && name.len() <= 128
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')),
        "connection name must be 1-128 bytes of letters, numbers, spaces, dots, underscores, or dashes"
    );
    ensure!(
        !is_local_alias(name),
        "that name is reserved for local execution"
    );
    Ok(name.to_lowercase())
}

pub fn validate_connection_name(name: &str) -> Result<()> {
    name_key(name).map(|_| ())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyAuthentication {
    pub path: Option<PathBuf>,
    pub passphrase_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    pub endpoints: Vec<Endpoint>,
    pub default_auth: AuthMethod,
    pub password_id: Option<Uuid>,
    pub key: Option<KeyAuthentication>,
}

impl Connection {
    pub fn ordered_endpoints(&self) -> Vec<&Endpoint> {
        self.endpoints
            .iter()
            .filter(|e| !e.is_deferred())
            .chain(self.endpoints.iter().filter(|e| e.is_deferred()))
            .collect()
    }

    fn credential_ids(&self) -> Vec<Uuid> {
        self.password_id
            .into_iter()
            .chain(self.key.as_ref().and_then(|key| key.passphrase_id))
            .collect()
    }

    fn validate(&self) -> Result<()> {
        name_key(&self.name)?;
        ensure!(!self.endpoints.is_empty(), "connection has no endpoints");
        let mut seen = HashSet::new();
        for endpoint in &self.endpoints {
            endpoint.validate()?;
            ensure!(
                seen.insert(endpoint.display()),
                "connection has duplicate endpoints"
            );
        }
        ensure!(
            match self.default_auth {
                AuthMethod::Password => self.password_id.is_some(),
                AuthMethod::Key => self.key.is_some(),
            },
            "connection has no configured default authentication"
        );
        if let Some(key) = &self.key {
            ensure!(
                key.path.as_ref().is_none_or(|p| p.is_absolute()),
                "key path must be absolute"
            );
            ensure!(
                key.path.is_some() || key.passphrase_id.is_none(),
                "SSH-agent authentication cannot store a passphrase"
            );
        }
        Ok(())
    }
}

// Deliberately neither Debug nor Serialize: secret input must never enter logs/events.
pub enum Credentials {
    Password(String),
    Key {
        path: Option<PathBuf>,
        passphrase: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryCommand {
    List,
    Show(String),
    Add {
        name: String,
        endpoints: Vec<Endpoint>,
        auth: AuthMethod,
    },
    Update {
        name: String,
        endpoints: Vec<Endpoint>,
    },
    Rename {
        name: String,
        new_name: String,
    },
    Auth {
        name: String,
        auth: AuthMethod,
    },
    Remove(String),
}

impl RegistryCommand {
    pub fn parse(input: &str) -> Result<Self> {
        let args = shlex::split(input).context("invalid quoting in /ssh command")?;
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let command = match args.as_slice() {
            [] | ["list"] => Self::List,
            ["show", name] => Self::Show((*name).to_owned()),
            ["add", endpoints, auth, name] => Self::Add {
                name: (*name).to_owned(),
                endpoints: parse_endpoints(endpoints)?,
                auth: AuthMethod::parse(auth)?,
            },
            ["update", name, endpoints] => Self::Update {
                name: (*name).to_owned(),
                endpoints: parse_endpoints(endpoints)?,
            },
            ["rename", name, new_name] => {
                name_key(new_name)?;
                Self::Rename {
                    name: (*name).to_owned(),
                    new_name: (*new_name).to_owned(),
                }
            }
            ["auth", name, auth] => Self::Auth {
                name: (*name).to_owned(),
                auth: AuthMethod::parse(auth)?,
            },
            ["remove", name] => Self::Remove((*name).to_owned()),
            _ => bail!("{HELP}"),
        };
        match &command {
            Self::List => {}
            Self::Show(name)
            | Self::Remove(name)
            | Self::Add { name, .. }
            | Self::Update { name, .. }
            | Self::Rename { name, .. }
            | Self::Auth { name, .. } => {
                name_key(name)?;
            }
        }
        Ok(command)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    version: u8,
    connections: BTreeMap<String, Connection>,
}

#[derive(Clone)]
pub struct Registry {
    home: PathBuf,
    keyring: Arc<dyn KeyringStore>,
}

impl Registry {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            keyring: best_keyring_store(),
        }
    }

    fn load(&self) -> Result<RegistryFile> {
        let path = self.home.join(FILENAME);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        ensure!(!path.is_symlink(), "SSH registry must not be a symlink");
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RegistryFile {
                    version: 1,
                    connections: BTreeMap::new(),
                });
            }
            Err(error) => return Err(error).context("cannot read SSH registry"),
        };
        ensure!(
            file.metadata()?.is_file(),
            "SSH registry must be a regular file"
        );
        let mut bytes = Vec::new();
        file.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_REGISTRY_BYTES,
            "SSH registry exceeds its 4 MiB storage budget"
        );
        let registry: RegistryFile = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("SSH registry is malformed; it has not been replaced"))?;
        ensure!(registry.version == 1, "unsupported SSH registry version");
        let mut identities = HashSet::new();
        for (key, connection) in &registry.connections {
            connection.validate()?;
            ensure!(
                name_key(&connection.name)? == *key,
                "SSH registry name mismatch"
            );
            ensure!(
                identities.insert(connection.id),
                "duplicate SSH connection identity"
            );
        }
        Ok(registry)
    }

    fn locked(&self) -> Result<(File, RegistryFile)> {
        std::fs::create_dir_all(&self.home)?;
        let path = self.home.join("ssh-connections.lock");
        ensure!(
            !path.is_symlink(),
            "SSH registry lock must not be a symlink"
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(path)?;
        let start = Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => {
                    return Err(error).context("SSH registry is busy or cannot be locked");
                }
            }
        }
        Ok((lock, self.load()?))
    }

    fn write(&self, registry: &RegistryFile) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(registry)?;
        ensure!(
            bytes.len() as u64 <= MAX_REGISTRY_BYTES,
            "SSH registry exceeds its 4 MiB storage budget"
        );
        let mut file = tempfile::NamedTempFile::new_in(&self.home)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.home.join(FILENAME))
            .context("cannot save SSH registry")?;
        Ok(())
    }

    fn account(&self, connection: Uuid, credential: Uuid) -> String {
        format!(
            "{}|{connection}|{credential}",
            store_key_for_code_home("ssh", &self.home)
        )
    }

    fn store_secret(&self, connection: Uuid, value: &str) -> Result<Uuid> {
        ensure!(!value.is_empty(), "credential must not be empty");
        let id = Uuid::new_v4();
        self.keyring.save(CREDENTIAL_SERVICE, &self.account(connection, id), value)
            .map_err(|_| anyhow::anyhow!("secure SSH credential storage is unavailable; no plaintext fallback is permitted"))?;
        Ok(id)
    }

    fn delete_secret(&self, connection: Uuid, credential: Uuid) -> Result<()> {
        self.keyring
            .delete(CREDENTIAL_SERVICE, &self.account(connection, credential))
            .map_err(|_| anyhow::anyhow!("SSH credential cleanup failed in the secure store"))?;
        Ok(())
    }

    fn apply_credentials(
        &self,
        connection: &mut Connection,
        credentials: Credentials,
    ) -> Result<()> {
        match credentials {
            Credentials::Password(value) => {
                connection.password_id = Some(self.store_secret(connection.id, &value)?);
                connection.default_auth = AuthMethod::Password;
            }
            Credentials::Key { path, passphrase } => {
                let path = path
                    .map(|p| p.canonicalize())
                    .transpose()
                    .context("SSH key file does not exist")?;
                ensure!(
                    path.as_ref().is_none_or(|p| p.is_file()),
                    "SSH key path must be a regular file"
                );
                ensure!(
                    path.is_some() || passphrase.is_none(),
                    "SSH-agent authentication does not accept a passphrase"
                );
                let passphrase_id = passphrase
                    .as_deref()
                    .map(|value| self.store_secret(connection.id, value))
                    .transpose()?;
                connection.key = Some(KeyAuthentication {
                    path,
                    passphrase_id,
                });
                connection.default_auth = AuthMethod::Key;
            }
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Connection>> {
        Ok(self.load()?.connections.into_values().collect())
    }

    pub fn get(&self, name: &str) -> Result<Connection> {
        self.load()?
            .connections
            .remove(&name_key(name)?)
            .context("SSH connection is not registered")
    }

    /// Resolve one already-configured authentication method for transport use.
    ///
    /// The returned value intentionally keeps secrets out of `Debug` and
    /// serialization paths. Callers should hold it only for the duration of a
    /// transport attempt.
    pub fn credentials(
        &self,
        connection: &Connection,
        requested_auth: Option<AuthMethod>,
    ) -> Result<Credentials> {
        let auth = requested_auth.unwrap_or(connection.default_auth);
        match auth {
            AuthMethod::Password => {
                let credential = connection
                    .password_id
                    .context("password authentication is not configured for this connection")?;
                let value = self
                    .keyring
                    .load(CREDENTIAL_SERVICE, &self.account(connection.id, credential))
                    .map_err(|_| anyhow::anyhow!("secure SSH credential storage is unavailable"))?
                    .context("saved SSH password is unavailable")?;
                Ok(Credentials::Password(value))
            }
            AuthMethod::Key => {
                let key = connection
                    .key
                    .as_ref()
                    .context("key authentication is not configured for this connection")?;
                let passphrase = key
                    .passphrase_id
                    .map(|credential| {
                        self.keyring
                            .load(CREDENTIAL_SERVICE, &self.account(connection.id, credential))
                            .map_err(|_| anyhow::anyhow!("secure SSH credential storage is unavailable"))?
                            .context("saved SSH key passphrase is unavailable")
                    })
                    .transpose()?;
                Ok(Credentials::Key {
                    path: key.path.clone(),
                    passphrase,
                })
            }
        }
    }

    pub fn add(
        &self,
        name: &str,
        endpoints: Vec<Endpoint>,
        credentials: Credentials,
    ) -> Result<Connection> {
        let key = name_key(name)?;
        let (_lock, mut registry) = self.locked()?;
        ensure!(
            !registry.connections.contains_key(&key),
            "SSH connection already exists; use update or auth"
        );
        let mut connection = Connection {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            endpoints,
            default_auth: AuthMethod::Key,
            password_id: None,
            key: Some(KeyAuthentication {
                path: None,
                passphrase_id: None,
            }),
        };
        connection.validate()?;
        connection.key = None;
        self.apply_credentials(&mut connection, credentials)?;
        registry.connections.insert(key, connection.clone());
        if let Err(error) = self.write(&registry) {
            for id in connection.credential_ids() {
                self.delete_secret(connection.id, id)
                    .context("registration failed and credential rollback failed")?;
            }
            return Err(error);
        }
        Ok(connection)
    }

    pub fn update_endpoints(&self, name: &str, endpoints: Vec<Endpoint>) -> Result<()> {
        let (_lock, mut registry) = self.locked()?;
        let connection = registry
            .connections
            .get_mut(&name_key(name)?)
            .context("SSH connection is not registered")?;
        connection.endpoints = endpoints;
        connection.validate()?;
        self.write(&registry)
    }

    pub fn rename(&self, name: &str, new_name: &str) -> Result<()> {
        let old_key = name_key(name)?;
        let new_key = name_key(new_name)?;
        let (_lock, mut registry) = self.locked()?;
        ensure!(
            old_key == new_key || !registry.connections.contains_key(&new_key),
            "SSH connection name already exists"
        );
        let mut connection = registry
            .connections
            .remove(&old_key)
            .context("SSH connection is not registered")?;
        connection.name = new_name.to_owned();
        registry.connections.insert(new_key, connection);
        self.write(&registry)
    }

    pub fn edit(
        &self,
        expected: &Connection,
        new_name: &str,
        endpoints: Vec<Endpoint>,
    ) -> Result<()> {
        let old_key = name_key(&expected.name)?;
        let new_key = name_key(new_name)?;
        let (_lock, mut registry) = self.locked()?;
        let current = registry
            .connections
            .get(&old_key)
            .context("SSH connection is not registered")?;
        ensure!(
            current == expected,
            "SSH connection changed while editing; refresh and retry"
        );
        ensure!(
            old_key == new_key || !registry.connections.contains_key(&new_key),
            "SSH connection name already exists"
        );
        let mut edited = current.clone();
        edited.name = new_name.to_owned();
        edited.endpoints = endpoints;
        edited.validate()?;
        registry.connections.remove(&old_key);
        registry.connections.insert(new_key, edited);
        self.write(&registry)
    }

    pub fn set_auth(&self, expected: &Connection, credentials: Credentials) -> Result<()> {
        let (_lock, mut registry) = self.locked()?;
        let connection = registry
            .connections
            .get_mut(&name_key(&expected.name)?)
            .context("SSH connection is not registered")?;
        ensure!(
            *connection == *expected,
            "SSH connection changed while credentials were being entered; reopen the form"
        );
        let old_ids = connection.credential_ids();
        self.apply_credentials(connection, credentials)?;
        let id = connection.id;
        let new_ids = connection.credential_ids();
        if let Err(error) = self.write(&registry) {
            for credential in new_ids.iter().filter(|c| !old_ids.contains(c)) {
                self.delete_secret(id, *credential)
                    .context("authentication save failed and credential rollback failed")?;
            }
            return Err(error);
        }
        for credential in old_ids.iter().filter(|c| !new_ids.contains(c)) {
            self.delete_secret(id, *credential)
                .context("new authentication saved, but old credential cleanup failed")?;
        }
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        self.remove_inner(name, None)
    }

    pub fn remove_expected(&self, expected: &Connection) -> Result<()> {
        self.remove_inner(&expected.name, Some(expected))
    }

    fn remove_inner(&self, name: &str, expected: Option<&Connection>) -> Result<()> {
        let (_lock, mut registry) = self.locked()?;
        if let Some(expected) = expected {
            ensure!(
                registry.connections.get(&name_key(name)?) == Some(expected),
                "SSH connection changed while confirming removal; refresh and retry"
            );
        }
        let connection = registry
            .connections
            .remove(&name_key(name)?)
            .context("SSH connection is not registered")?;
        self.write(&registry)?;
        for credential in connection.credential_ids() {
            self.delete_secret(connection.id, credential)
                .context("connection removed, but its credential remains in the secure store")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_keyring_store::CredentialStoreError;
    use std::sync::Mutex;

    #[test]
    fn settings_edits_are_atomic_and_stale_removal_is_rejected() {
        let (_directory, registry, _) = fixture();
        let original = registry.add("first", endpoints(), agent()).unwrap();
        registry.add("second", endpoints(), agent()).unwrap();
        assert!(
            registry
                .edit(&original, "second", parse_endpoints("u@new-host").unwrap())
                .is_err()
        );
        assert_eq!(registry.get("first").unwrap(), original);
        registry
            .edit(&original, "renamed", parse_endpoints("u@new-host").unwrap())
            .unwrap();
        assert_eq!(registry.get("renamed").unwrap().id, original.id);
        assert!(registry.remove_expected(&original).is_err());
        let edited = registry.get("renamed").unwrap();
        registry.update_endpoints("renamed", endpoints()).unwrap();
        assert!(registry.remove_expected(&edited).is_err());
        registry
            .remove_expected(&registry.get("renamed").unwrap())
            .unwrap();
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[derive(Debug, Default)]
    struct Keyring(Mutex<BTreeMap<String, String>>);
    impl KeyringStore for Keyring {
        fn load(
            &self,
            _: &str,
            account: &str,
        ) -> std::result::Result<Option<String>, CredentialStoreError> {
            Ok(self.0.lock().unwrap().get(account).cloned())
        }
        fn save(
            &self,
            _: &str,
            account: &str,
            value: &str,
        ) -> std::result::Result<(), CredentialStoreError> {
            self.0
                .lock()
                .unwrap()
                .insert(account.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(
            &self,
            _: &str,
            account: &str,
        ) -> std::result::Result<bool, CredentialStoreError> {
            Ok(self.0.lock().unwrap().remove(account).is_some())
        }
    }
    fn fixture() -> (tempfile::TempDir, Registry, Arc<Keyring>) {
        let directory = tempfile::tempdir().unwrap();
        let keyring = Arc::new(Keyring::default());
        let registry = Registry {
            home: directory.path().to_owned(),
            keyring: keyring.clone(),
        };
        (directory, registry, keyring)
    }
    fn agent() -> Credentials {
        Credentials::Key {
            path: None,
            passphrase: None,
        }
    }
    fn endpoints() -> Vec<Endpoint> {
        parse_endpoints("user@10.0.0.5,user@100.64.0.1").unwrap()
    }

    #[test]
    fn dynamically_adds_updates_renames_and_removes_connections() {
        let (_directory, registry, _) = fixture();
        assert!(registry.list().unwrap().is_empty());
        for index in 0..12 {
            registry
                .add(&format!("PC {index}"), endpoints(), agent())
                .unwrap();
        }
        assert_eq!(registry.list().unwrap().len(), 12);
        let id = registry.get("pc 0").unwrap().id;
        registry
            .update_endpoints("PC 0", parse_endpoints("user@new-host:2222").unwrap())
            .unwrap();
        registry.rename("PC 0", "Office PC").unwrap();
        assert_eq!(registry.get("office pc").unwrap().id, id);
        assert_eq!(registry.get("Office PC").unwrap().endpoints[0].port, 2222);
        registry.remove("Office PC").unwrap();
        assert_eq!(registry.list().unwrap().len(), 11);
        assert!(registry.get("Office PC").is_err());
    }

    #[test]
    fn defers_only_cgnat_range_stably_without_rewriting_saved_order() {
        let (_dir, registry, _) = fixture();
        let entered = "u@100.64.0.0,u@10.0.0.5,u@100.127.255.255,u@100.63.255.255,u@100.128.0.0,u@other-host,u@100.100.1.2";
        let connection = registry
            .add("test", parse_endpoints(entered).unwrap(), agent())
            .unwrap();
        let ordered = connection
            .ordered_endpoints()
            .iter()
            .map(|e| e.host.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            [
                "10.0.0.5",
                "100.63.255.255",
                "100.128.0.0",
                "other-host",
                "100.64.0.0",
                "100.127.255.255",
                "100.100.1.2"
            ]
        );
        assert_eq!(
            registry.get("test").unwrap().endpoints,
            parse_endpoints(entered).unwrap()
        );
        assert!(!Endpoint::parse("u@100.1.1.1").unwrap().is_deferred());
        assert!(
            Endpoint::parse("u@[::ffff:100.64.0.1]")
                .unwrap()
                .is_deferred()
        );
    }

    #[test]
    fn endpoint_validation_and_reserved_names_are_enforced() {
        let (_dir, registry, _) = fixture();
        for bad in [
            "",
            "host",
            "u:password@host",
            "u@host/path",
            "u@host?x=1",
            "u@host#fragment",
            "u@host:0",
            "u@-host",
            "u@100.999.1.1",
            "-u@host",
            "u@host,",
            "u@host,u@host",
        ] {
            assert!(parse_endpoints(bad).is_err(), "accepted {bad}");
        }
        for name in [
            "self",
            "SELF",
            "127.0.0.1",
            "0.0.0.0",
            "../bad",
            "",
            "bad\nname",
        ] {
            assert!(registry.add(name, endpoints(), agent()).is_err());
        }
        assert_eq!(
            Endpoint::parse("ssh://user@[2001:db8::1]:2222")
                .unwrap()
                .display(),
            "user@[2001:db8::1]:2222"
        );
    }

    #[test]
    fn credentials_never_enter_metadata_and_are_removed_from_keyring() {
        let (directory, registry, keyring) = fixture();
        let original = registry
            .add(
                "test",
                endpoints(),
                Credentials::Password("unique-secret-one".into()),
            )
            .unwrap();
        registry.rename("test", "Renamed").unwrap();
        registry
            .set_auth(
                &registry.get("Renamed").unwrap(),
                Credentials::Password("unique-secret-two".into()),
            )
            .unwrap();
        assert_eq!(registry.get("Renamed").unwrap().id, original.id);
        assert_eq!(keyring.0.lock().unwrap().len(), 1);
        let data = std::fs::read_to_string(directory.path().join(FILENAME)).unwrap();
        assert!(!data.contains("unique-secret"));
        registry
            .set_auth(&registry.get("Renamed").unwrap(), agent())
            .unwrap();
        assert!(registry.get("Renamed").unwrap().password_id.is_some());
        registry.remove("Renamed").unwrap();
        assert!(keyring.0.lock().unwrap().is_empty());
    }

    #[test]
    fn duplicate_or_failed_changes_leave_registry_unchanged() {
        let (directory, registry, _) = fixture();
        registry.add("test", endpoints(), agent()).unwrap();
        let before = std::fs::read(directory.path().join(FILENAME)).unwrap();
        assert!(registry.add("TEST", endpoints(), agent()).is_err());
        assert!(registry.update_endpoints("test", vec![]).is_err());
        assert!(registry.rename("test", "self").is_err());
        assert_eq!(
            std::fs::read(directory.path().join(FILENAME)).unwrap(),
            before
        );
    }

    #[test]
    fn stale_credential_form_cannot_change_replaced_or_edited_connection() {
        let (_directory, registry, keyring) = fixture();
        let original = registry.add("pc", endpoints(), agent()).unwrap();
        registry
            .update_endpoints("pc", parse_endpoints("user@changed-host").unwrap())
            .unwrap();
        assert!(
            registry
                .set_auth(&original, Credentials::Password("never-save-this".into()))
                .is_err()
        );
        registry.remove("pc").unwrap();
        registry.add("pc", endpoints(), agent()).unwrap();
        assert!(
            registry
                .set_auth(&original, Credentials::Password("never-save-this".into()))
                .is_err()
        );
        assert!(keyring.0.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_registry_is_not_overwritten() {
        let (directory, registry, _) = fixture();
        let path = directory.path().join(FILENAME);
        std::fs::write(&path, b"not json").unwrap();
        assert!(registry.add("test", endpoints(), agent()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"not json");
    }

    #[test]
    fn unavailable_secure_storage_never_creates_a_plaintext_credential() {
        #[derive(Debug)]
        struct Unavailable;
        impl KeyringStore for Unavailable {
            fn load(
                &self,
                _: &str,
                _: &str,
            ) -> std::result::Result<Option<String>, CredentialStoreError> {
                Ok(None)
            }
            fn save(
                &self,
                _: &str,
                _: &str,
                _: &str,
            ) -> std::result::Result<(), CredentialStoreError> {
                Err(CredentialStoreError::new(keyring::Error::NoEntry))
            }
            fn delete(&self, _: &str, _: &str) -> std::result::Result<bool, CredentialStoreError> {
                Ok(false)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let registry = Registry {
            home: directory.path().to_owned(),
            keyring: Arc::new(Unavailable),
        };
        let error = registry
            .add(
                "test",
                endpoints(),
                Credentials::Password("never-persist-plaintext".into()),
            )
            .unwrap_err();
        assert!(!error.to_string().contains("never-persist-plaintext"));
        assert!(!directory.path().join(FILENAME).exists());
        assert!(registry.list().unwrap().is_empty());
    }

    #[test]
    fn oversized_registry_is_rejected_without_mutation() {
        let (directory, registry, _) = fixture();
        let path = directory.path().join(FILENAME);
        let file = File::create(&path).unwrap();
        file.set_len(MAX_REGISTRY_BYTES + 1).unwrap();
        assert!(registry.add("test", endpoints(), agent()).is_err());
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            MAX_REGISTRY_BYTES + 1
        );
    }

    #[test]
    fn concurrent_writers_do_not_lose_connections() {
        let (_directory, registry, _) = fixture();
        let threads = (0..8)
            .map(|index| {
                let registry = registry.clone();
                std::thread::spawn(move || {
                    registry
                        .add(&format!("pc{index}"), endpoints(), agent())
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(registry.list().unwrap().len(), 8);
    }

    #[test]
    fn command_parser_supports_quoted_names_and_rejects_inline_secrets() {
        assert!(
            matches!(RegistryCommand::parse("add u@host,u@100.64.0.1 key 'Office PC'").unwrap(), RegistryCommand::Add { name, .. } if name == "Office PC")
        );
        for input in [
            "add u:secret@host pwd pc",
            "add u@host password-value pc",
            "add u@host pwd pc extra",
            "remove",
            "rename a",
            "auth self pwd",
            "show 'unterminated",
        ] {
            assert!(RegistryCommand::parse(input).is_err());
        }
        assert_eq!(RegistryCommand::parse("").unwrap(), RegistryCommand::List);
    }

    #[cfg(unix)]
    #[test]
    fn registry_is_private_and_symlinks_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (directory, registry, _) = fixture();
        registry.add("test", endpoints(), agent()).unwrap();
        let path = directory.path().join(FILENAME);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let target = directory.path().join("saved");
        std::fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(registry.list().is_err());
        assert!(registry.add("other", endpoints(), agent()).is_err());
    }
}
