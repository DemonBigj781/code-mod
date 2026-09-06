use async_trait::async_trait;
use code_core::AuthManager;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use std::fmt;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::watch;

pub(crate) const REMOTE_CONTROL_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";

#[derive(Clone)]
pub(crate) struct RemoteControlAuth {
    access_token: String,
    account_id: String,
}

impl RemoteControlAuth {
    #[cfg(test)]
    pub(crate) fn for_testing(access_token: &str, account_id: &str) -> Self {
        Self {
            access_token: access_token.to_string(),
            account_id: account_id.to_string(),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn request_headers(&self) -> io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|error| {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid remote control authorization header: {error}"),
                )
            })?;
        let account_id = HeaderValue::from_str(&self.account_id).map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid remote control account id header: {error}"),
            )
        })?;
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(REMOTE_CONTROL_ACCOUNT_ID_HEADER, account_id);
        Ok(headers)
    }
}

impl fmt::Debug for RemoteControlAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteControlAuth")
            .field("access_token", &"[REDACTED]")
            .field("account_id", &self.account_id)
            .finish()
    }
}

#[async_trait]
pub(crate) trait RemoteControlAuthProvider: Send + Sync {
    async fn load(&self) -> io::Result<RemoteControlAuth>;
    async fn recover_unauthorized(&self) -> io::Result<bool>;
    fn subscribe(&self) -> watch::Receiver<u64>;
}

pub(crate) struct CoreRemoteControlAuthProvider {
    auth_manager: Arc<AuthManager>,
    loaded_revision: AtomicU64,
}

impl CoreRemoteControlAuthProvider {
    pub fn new(auth_manager: Arc<AuthManager>) -> Self {
        let loaded_revision = AtomicU64::new(auth_manager.auth_revision());
        Self {
            auth_manager,
            loaded_revision,
        }
    }
}

#[async_trait]
impl RemoteControlAuthProvider for CoreRemoteControlAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        let mut reloaded = false;
        let auth = loop {
            let Some(auth) = self.auth_manager.auth() else {
                if reloaded {
                    return Err(io::Error::new(
                        ErrorKind::PermissionDenied,
                        "remote control requires ChatGPT authentication",
                    ));
                }
                self.auth_manager.reload();
                reloaded = true;
                continue;
            };
            if !auth.uses_codex_backend() {
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "remote control requires ChatGPT authentication; API key auth is not supported",
                ));
            }
            if auth.get_account_id().is_none() && !reloaded {
                self.auth_manager.reload();
                reloaded = true;
                continue;
            }
            break auth;
        };

        let account_id = auth.get_account_id().ok_or_else(|| {
            io::Error::new(
                ErrorKind::WouldBlock,
                "remote control enrollment is waiting for a ChatGPT account id",
            )
        })?;
        let access_token = auth.get_token().await?;
        self.loaded_revision
            .store(self.auth_manager.auth_revision(), Ordering::Release);
        Ok(RemoteControlAuth {
            access_token,
            account_id,
        })
    }

    async fn recover_unauthorized(&self) -> io::Result<bool> {
        let observed_revision = self.loaded_revision.load(Ordering::Acquire);
        self.auth_manager
            .recover_unauthorized_since(observed_revision)
            .await
            .map_err(io::Error::other)
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.auth_manager.auth_change_receiver()
    }
}
