#[allow(dead_code)]
pub(crate) mod auth;
#[cfg(test)]
mod auth_tests;
#[allow(dead_code)]
pub(crate) mod clients;
#[cfg(test)]
mod clients_tests;
#[allow(dead_code)]
pub(crate) mod client_tracker;
#[cfg(test)]
mod client_tracker_tests;
mod desired_state;
#[allow(dead_code)]
pub(crate) mod enroll;
#[cfg(test)]
mod enroll_tests;
#[allow(dead_code)]
pub(crate) mod host_device;
#[allow(dead_code)]
pub(crate) mod protocol;
#[cfg(test)]
mod protocol_tests;
#[allow(dead_code)]
pub(crate) mod server_api;
#[cfg(test)]
mod server_api_tests;
#[allow(dead_code)]
pub(crate) mod segment;
#[cfg(test)]
mod segment_tests;
#[allow(dead_code)]
pub(crate) mod state;
#[allow(dead_code)]
pub(crate) mod websocket;
#[cfg(test)]
mod websocket_tests;

#[cfg(test)]
mod tests;

use self::auth::RemoteControlAuthProvider;
use self::desired_state::RemoteControlDesiredState;
use self::enroll::RemoteControlEnrollment;
use self::enroll::RemoteControlEnrollmentSelection;
use self::enroll::resolve_remote_control_enrollment;
use self::enroll::update_persisted_remote_control_enrollment;
use self::host_device::HostDevice;
use self::protocol::RemoteControlTarget;
use self::protocol::normalize_remote_control_url;
use self::state::RemoteControlState;
use self::websocket::RemoteControlWebsocketConfig;
use self::websocket::run_remote_control_websocket;
use crate::transport::TransportEvent;
use code_app_server_protocol::RemoteControlClientsListParams;
use code_app_server_protocol::RemoteControlClientsListResponse;
use code_app_server_protocol::RemoteControlClientsRevokeParams;
use code_app_server_protocol::RemoteControlClientsRevokeResponse;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use code_app_server_protocol::RemoteControlPairingStatusResponse;
use code_app_server_protocol::RemoteControlStatusChangedNotification;
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
pub(crate) struct RemoteControlStartConfig {
    pub(crate) remote_control_url: String,
    pub(crate) installation_id: String,
    pub(crate) host: HostDevice,
    pub(crate) policy: RemoteControlPolicy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RemoteControlPolicy {
    #[default]
    Allowed,
    DisabledByRequirements,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlStartupMode {
    ResolvePersisted,
    DisabledEphemeral,
    EnabledEphemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct RemoteControlUnavailable;

impl fmt::Display for RemoteControlUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote control cannot be enabled because sqlite state is unavailable")
    }
}

impl Error for RemoteControlUnavailable {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct RemoteControlDisabledByRequirements;

impl fmt::Display for RemoteControlDisabledByRequirements {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote control is disabled by managed requirements")
    }
}

impl Error for RemoteControlDisabledByRequirements {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RemoteControlEnableError {
    Unavailable(RemoteControlUnavailable),
    DisabledByRequirements(RemoteControlDisabledByRequirements),
}

impl fmt::Display for RemoteControlEnableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => error.fmt(formatter),
            Self::DisabledByRequirements(error) => error.fmt(formatter),
        }
    }
}

impl Error for RemoteControlEnableError {}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct RemoteControlHandle {
    policy: RemoteControlPolicy,
    desired_state_tx: Arc<watch::Sender<RemoteControlDesiredState>>,
    desired_state_transition_lock: Arc<Semaphore>,
    desired_state_persistence_lock: Arc<Semaphore>,
    status_tx: Arc<watch::Sender<RemoteControlStatusChangedNotification>>,
    state: Option<Arc<RemoteControlState>>,
    target: RemoteControlTarget,
    installation_id: String,
    host: HostDevice,
    current_enrollment: Arc<Mutex<Option<RemoteControlEnrollment>>>,
    app_server_client_name: Arc<Mutex<Option<String>>>,
    auth_provider: Arc<dyn RemoteControlAuthProvider>,
}

#[allow(dead_code)]
impl RemoteControlHandle {
    pub(crate) fn ensure_remote_control_allowed(
        &self,
    ) -> Result<(), RemoteControlDisabledByRequirements> {
        match self.policy {
            RemoteControlPolicy::Allowed => Ok(()),
            RemoteControlPolicy::DisabledByRequirements => {
                Err(RemoteControlDisabledByRequirements)
            }
        }
    }

    pub(crate) fn status(&self) -> RemoteControlStatusChangedNotification {
        self.status_tx.borrow().clone()
    }

    pub(crate) fn status_receiver(
        &self,
    ) -> watch::Receiver<RemoteControlStatusChangedNotification> {
        self.status_tx.subscribe()
    }

    pub(crate) async fn resolve_persisted_preference(
        &self,
        app_server_client_name: Option<&str>,
    ) -> io::Result<bool> {
        if self.ensure_remote_control_allowed().is_err() {
            return Ok(false);
        }
        let _transition = self
            .desired_state_transition_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        self.record_app_server_client_name(app_server_client_name)
            .await?;
        if !matches!(*self.desired_state_tx.borrow(), RemoteControlDesiredState::Unknown) {
            return Ok(self.desired_state_tx.borrow().is_enabled());
        }
        let state = self.state.as_deref().ok_or_else(remote_control_unavailable)?;
        loop {
            let auth = self.auth_provider.load().await?;
            let client_name = self.app_server_client_name.lock().await.clone();
            let enrollment = state
                .get_enrollment(
                    &self.target.websocket_url,
                    auth.account_id(),
                    client_name.as_deref(),
                )
                .await
                .map_err(io::Error::other)?;
            let current_auth = self.auth_provider.load().await?;
            if current_auth.account_id() != auth.account_id() {
                continue;
            }
            let enabled = enrollment
                .as_ref()
                .and_then(|enrollment| enrollment.remote_control_enabled)
                == Some(true);
            if let Some(enrollment) = enrollment {
                *self.current_enrollment.lock().await = Some(RemoteControlEnrollment {
                    remote_control_target: self.target.clone(),
                    account_id: enrollment.account_id,
                    environment_id: enrollment.environment_id,
                    server_id: enrollment.server_id,
                    server_name: self.host.name.clone(),
                    remote_control_token: None,
                    expires_at: None,
                    next_refresh_at: None,
                });
            }
            let desired_state = if enabled {
                RemoteControlDesiredState::Enabled {
                    persistence_preference: Some(true),
                }
            } else {
                RemoteControlDesiredState::Disabled
            };
            self.desired_state_tx.send_if_modified(|state| {
                if !matches!(*state, RemoteControlDesiredState::Unknown) {
                    return false;
                }
                *state = desired_state;
                true
            });
            let enabled = self.desired_state_tx.borrow().is_enabled();
            self.publish_status(if enabled {
                RemoteControlConnectionStatus::Connecting
            } else {
                RemoteControlConnectionStatus::Disabled
            });
            return Ok(enabled);
        }
    }

    pub(crate) async fn enable(
        &self,
        app_server_client_name: Option<&str>,
    ) -> io::Result<RemoteControlStatusChangedNotification> {
        self.ensure_remote_control_allowed_io()?;
        let _transition = self
            .desired_state_transition_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        self.record_app_server_client_name(app_server_client_name)
            .await?;
        let state = self.state.as_deref().ok_or_else(remote_control_unavailable)?;
        let app_server_client_name = self.app_server_client_name.lock().await.clone();
        let persistence_preference = self.desired_state_tx.borrow().persistence_preference();
        let mut current_enrollment = self.current_enrollment.lock().await;
        let enrollment = resolve_remote_control_enrollment(
            state,
            &self.target,
            self.auth_provider.as_ref(),
            &self.installation_id,
            &self.host,
            app_server_client_name.as_deref(),
            current_enrollment.as_ref(),
            persistence_preference,
            RemoteControlEnrollmentSelection::ReuseOrCreate,
        )
        .await?;
        let current_auth = self.auth_provider.load().await?;
        if current_auth.account_id() != enrollment.account_id {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "remote control account changed during enrollment",
            ));
        }
        let _persistence = self
            .desired_state_persistence_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        let updated = state
            .set_enabled(
                &self.target.websocket_url,
                &enrollment.account_id,
                app_server_client_name.as_deref(),
                true,
            )
            .await
            .map_err(io::Error::other)?;
        if updated == 0 {
            update_persisted_remote_control_enrollment(
                state,
                &self.target,
                &enrollment.account_id,
                app_server_client_name.as_deref(),
                Some(&enrollment),
                Some(true),
            )
            .await?;
        }
        *current_enrollment = Some(enrollment.clone());
        self.enable_with_preference(Some(true));
        self.publish_environment_id(Some(enrollment.environment_id));
        Ok(self.status())
    }

    pub(crate) fn enable_ephemeral(
        &self,
    ) -> Result<RemoteControlStatusChangedNotification, RemoteControlEnableError> {
        self.ensure_remote_control_allowed()
            .map_err(RemoteControlEnableError::DisabledByRequirements)?;
        if self.state.is_none() {
            return Err(RemoteControlEnableError::Unavailable(
                RemoteControlUnavailable,
            ));
        }
        let preference = match *self.desired_state_tx.borrow() {
            RemoteControlDesiredState::Enabled {
                persistence_preference: Some(true),
            } => Some(true),
            RemoteControlDesiredState::Unknown
            | RemoteControlDesiredState::Disabled
            | RemoteControlDesiredState::Enabled { .. } => None,
        };
        Ok(self.enable_with_preference(preference))
    }

    pub(crate) async fn disable(
        &self,
        app_server_client_name: Option<&str>,
    ) -> io::Result<RemoteControlStatusChangedNotification> {
        self.ensure_remote_control_allowed_io()?;
        let _transition = self
            .desired_state_transition_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        let _persistence = self
            .desired_state_persistence_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        self.persist_preference(app_server_client_name, false).await?;
        Ok(self.transition_disabled())
    }

    pub(crate) async fn disable_ephemeral(&self) -> RemoteControlStatusChangedNotification {
        let _transition = self
            .desired_state_transition_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        let _persistence = self
            .desired_state_persistence_lock
            .acquire()
            .await
            .unwrap_or_else(|_| unreachable!());
        self.transition_disabled()
    }

    pub(crate) async fn start_pairing(
        &self,
        params: RemoteControlPairingStartParams,
        app_server_client_name: Option<&str>,
    ) -> io::Result<RemoteControlPairingStartResponse> {
        self.ensure_remote_control_allowed_io()?;
        if !self.desired_state_tx.borrow().is_enabled() {
            return Err(pairing_disabled_error());
        }
        self.record_app_server_client_name(app_server_client_name)
            .await?;
        let app_server_client_name = self.app_server_client_name.lock().await.clone();
        let mut current_enrollment = self.current_enrollment.lock().await;
        let preference = self.desired_state_tx.borrow().persistence_preference();
        let mut selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
        let mut retried = false;
        loop {
            let enrollment = resolve_remote_control_enrollment(
                self.state.as_deref().ok_or_else(remote_control_unavailable)?,
                &self.target,
                self.auth_provider.as_ref(),
                &self.installation_id,
                &self.host,
                app_server_client_name.as_deref(),
                current_enrollment.as_ref(),
                preference,
                selection,
            )
            .await?;
            *current_enrollment = Some(enrollment.clone());
            match enrollment.start_pairing(params.clone()).await {
                Ok(response) => {
                    let current_auth = self.auth_provider.load().await?;
                    if current_auth.account_id() != enrollment.account_id {
                        return Err(pairing_unavailable_error());
                    }
                    if !self.desired_state_tx.borrow().is_enabled() {
                        return Err(pairing_disabled_error());
                    }
                    return Ok(response);
                }
                Err(error)
                    if !retried && error.kind() == io::ErrorKind::PermissionDenied =>
                {
                    clear_server_token(&mut current_enrollment, &enrollment);
                    selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                    retried = true;
                }
                Err(error) if !retried && error.kind() == io::ErrorKind::NotFound => {
                    selection = RemoteControlEnrollmentSelection::ReplaceExisting;
                    retried = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) async fn pairing_status(
        &self,
        params: RemoteControlPairingStatusParams,
    ) -> io::Result<RemoteControlPairingStatusResponse> {
        self.ensure_remote_control_allowed_io()?;
        if !self.desired_state_tx.borrow().is_enabled() {
            return Err(pairing_disabled_error());
        }
        let app_server_client_name = self.app_server_client_name.lock().await.clone();
        let preference = self.desired_state_tx.borrow().persistence_preference();
        let mut current_enrollment = self.current_enrollment.lock().await;
        let auth = self.auth_provider.load().await?;
        if !current_enrollment
            .as_ref()
            .is_some_and(|enrollment| enrollment.account_id == auth.account_id())
        {
            return Err(pairing_unavailable_error());
        }
        let mut retried = false;
        loop {
            let enrollment = resolve_remote_control_enrollment(
                self.state.as_deref().ok_or_else(remote_control_unavailable)?,
                &self.target,
                self.auth_provider.as_ref(),
                &self.installation_id,
                &self.host,
                app_server_client_name.as_deref(),
                current_enrollment.as_ref(),
                preference,
                RemoteControlEnrollmentSelection::ReuseOrCreate,
            )
            .await?;
            *current_enrollment = Some(enrollment.clone());
            match enrollment.pairing_status(params.clone()).await {
                Ok(response) => {
                    let current_auth = self.auth_provider.load().await?;
                    if current_auth.account_id() != enrollment.account_id {
                        return Err(pairing_unavailable_error());
                    }
                    if !self.desired_state_tx.borrow().is_enabled() {
                        return Err(pairing_disabled_error());
                    }
                    return Ok(response);
                }
                Err(error)
                    if !retried && error.kind() == io::ErrorKind::PermissionDenied =>
                {
                    clear_server_token(&mut current_enrollment, &enrollment);
                    retried = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) async fn list_clients(
        &self,
        params: RemoteControlClientsListParams,
    ) -> io::Result<RemoteControlClientsListResponse> {
        self.ensure_remote_control_allowed_io()?;
        clients::list_remote_control_clients(
            &self.target.enroll_url,
            self.auth_provider.as_ref(),
            params,
        )
        .await
    }

    pub(crate) async fn revoke_client(
        &self,
        params: RemoteControlClientsRevokeParams,
    ) -> io::Result<RemoteControlClientsRevokeResponse> {
        self.ensure_remote_control_allowed_io()?;
        clients::revoke_remote_control_client(
            &self.target.enroll_url,
            self.auth_provider.as_ref(),
            params,
        )
        .await
    }

    fn ensure_remote_control_allowed_io(&self) -> io::Result<()> {
        self.ensure_remote_control_allowed()
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error))
    }

    fn enable_with_preference(
        &self,
        persistence_preference: Option<bool>,
    ) -> RemoteControlStatusChangedNotification {
        self.desired_state_tx
            .send_replace(RemoteControlDesiredState::Enabled {
                persistence_preference,
            });
        let status = self.status();
        if matches!(
            status.status,
            RemoteControlConnectionStatus::Connecting | RemoteControlConnectionStatus::Connected
        ) {
            status
        } else {
            self.publish_status(RemoteControlConnectionStatus::Connecting)
        }
    }

    fn transition_disabled(&self) -> RemoteControlStatusChangedNotification {
        self.desired_state_tx
            .send_replace(RemoteControlDesiredState::Disabled);
        self.publish_status(RemoteControlConnectionStatus::Disabled)
    }

    async fn persist_preference(
        &self,
        app_server_client_name: Option<&str>,
        enabled: bool,
    ) -> io::Result<()> {
        self.record_app_server_client_name(app_server_client_name)
            .await?;
        let state = self.state.as_deref().ok_or_else(remote_control_unavailable)?;
        let auth = self.auth_provider.load().await?;
        let client_name = self.app_server_client_name.lock().await.clone();
        state
            .set_enabled(
                &self.target.websocket_url,
                auth.account_id(),
                client_name.as_deref(),
                enabled,
            )
            .await
            .map_err(io::Error::other)?;
        Ok(())
    }

    async fn record_app_server_client_name(
        &self,
        app_server_client_name: Option<&str>,
    ) -> io::Result<()> {
        let Some(app_server_client_name) = app_server_client_name else {
            return Ok(());
        };
        let mut current = self.app_server_client_name.lock().await;
        if current.is_none() {
            *current = Some(app_server_client_name.to_string());
        }
        Ok(())
    }

    fn publish_status(
        &self,
        connection_status: RemoteControlConnectionStatus,
    ) -> RemoteControlStatusChangedNotification {
        self.status_tx.send_if_modified(|status| {
            let environment_id = if connection_status == RemoteControlConnectionStatus::Disabled {
                None
            } else {
                status.environment_id.clone()
            };
            if status.status == connection_status && status.environment_id == environment_id {
                return false;
            }
            status.status = connection_status;
            status.environment_id = environment_id;
            true
        });
        self.status()
    }

    fn publish_environment_id(&self, environment_id: Option<String>) {
        self.status_tx.send_if_modified(|status| {
            if status.environment_id == environment_id {
                return false;
            }
            status.environment_id = environment_id;
            true
        });
    }
}

#[allow(dead_code)]
pub(crate) async fn start_remote_control(
    config: RemoteControlStartConfig,
    state: Option<Arc<RemoteControlState>>,
    auth_provider: Arc<dyn RemoteControlAuthProvider>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown: CancellationToken,
    startup_mode: RemoteControlStartupMode,
) -> io::Result<(JoinHandle<io::Result<()>>, RemoteControlHandle)> {
    let target = normalize_remote_control_url(&config.remote_control_url)?;
    let desired_state = if config.policy == RemoteControlPolicy::DisabledByRequirements
        || state.is_none()
    {
        RemoteControlDesiredState::Disabled
    } else {
        match startup_mode {
            RemoteControlStartupMode::ResolvePersisted => RemoteControlDesiredState::Unknown,
            RemoteControlStartupMode::DisabledEphemeral => RemoteControlDesiredState::Disabled,
            RemoteControlStartupMode::EnabledEphemeral => RemoteControlDesiredState::Enabled {
                persistence_preference: None,
            },
        }
    };
    let initial_status = RemoteControlStatusChangedNotification {
        status: if desired_state.is_enabled() {
            RemoteControlConnectionStatus::Connecting
        } else {
            RemoteControlConnectionStatus::Disabled
        },
        server_name: config.host.name.clone(),
        installation_id: config.installation_id.clone(),
        environment_id: None,
    };
    let (desired_state_tx, desired_state_rx) = watch::channel(desired_state);
    let desired_state_tx = Arc::new(desired_state_tx);
    let (status_tx, _) = watch::channel(initial_status);
    let status_tx = Arc::new(status_tx);
    let current_enrollment = Arc::new(Mutex::new(None));
    let app_server_client_name = Arc::new(Mutex::new(None));
    let task = if let Some(state) = state.clone() {
        tokio::spawn(run_remote_control_websocket(RemoteControlWebsocketConfig {
            state,
            target: target.clone(),
            auth_provider: auth_provider.clone(),
            installation_id: config.installation_id.clone(),
            host: config.host.clone(),
            app_server_client_name: app_server_client_name.clone(),
            current_enrollment: current_enrollment.clone(),
            transport_event_tx,
            shutdown: shutdown.clone(),
            desired_state_rx,
            status_tx: status_tx.clone(),
        }))
    } else {
        tokio::spawn(async move {
            shutdown.cancelled().await;
            Ok(())
        })
    };
    let handle = RemoteControlHandle {
        policy: config.policy,
        desired_state_tx,
        desired_state_transition_lock: Arc::new(Semaphore::new(1)),
        desired_state_persistence_lock: Arc::new(Semaphore::new(1)),
        status_tx,
        state,
        target,
        installation_id: config.installation_id,
        host: config.host,
        current_enrollment,
        app_server_client_name,
        auth_provider,
    };
    Ok((task, handle))
}

#[allow(dead_code)]
fn remote_control_unavailable() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, RemoteControlUnavailable)
}

#[allow(dead_code)]
fn pairing_disabled_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "remote control pairing requires remote control to be enabled",
    )
}

#[allow(dead_code)]
fn pairing_unavailable_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotConnected,
        "remote control pairing is unavailable",
    )
}

fn clear_server_token(
    current_enrollment: &mut Option<RemoteControlEnrollment>,
    failed_enrollment: &RemoteControlEnrollment,
) {
    let Some(current_enrollment) = current_enrollment.as_mut() else {
        return;
    };
    if current_enrollment.account_id == failed_enrollment.account_id
        && current_enrollment.server_id == failed_enrollment.server_id
        && current_enrollment.environment_id == failed_enrollment.environment_id
        && current_enrollment.remote_control_token == failed_enrollment.remote_control_token
    {
        current_enrollment.remote_control_token = None;
        current_enrollment.expires_at = None;
        current_enrollment.next_refresh_at = None;
    }
}

#[cfg(test)]
mod state_tests;
