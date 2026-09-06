mod backend;
mod client;
mod process;
mod remote_control_client;
mod settings;

use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use serde::Deserialize;
use serde::Serialize;
use tokio::time::Instant;
use tokio::time::sleep;

pub use backend::BackendState;
pub use backend::ProcessBackend;
pub use backend::StartRequest;
pub use client::ProbeInfo;
pub use process::ProcessIdentity;
pub use process::validate_owned_path;

use process::PidBackend;
use remote_control_client::ControlSocketClient;
use settings::DaemonSettings;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleCommand {
    Start,
    Restart,
    Stop,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleStatus {
    AlreadyRunning,
    Started,
    Restarted,
    Stopped,
    NotRunning,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    Pid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleOutput {
    pub status: LifecycleStatus,
    pub backend: Option<BackendKind>,
    pub pid: Option<u32>,
    pub socket_path: PathBuf,
    pub cli_version: String,
    pub app_server_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteControlMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteControlStatus {
    Enabled,
    AlreadyEnabled,
    Disabled,
    AlreadyDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlReadyStatus {
    pub status: RemoteControlConnectionStatus,
    pub server_name: String,
    pub installation_id: String,
    pub environment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlOutput {
    pub status: RemoteControlStatus,
    pub backend: BackendKind,
    pub remote_control_enabled: bool,
    pub socket_path: PathBuf,
    pub cli_version: String,
    pub app_server_version: String,
    pub ready: RemoteControlReadyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlReadyOutput {
    pub daemon: LifecycleOutput,
    pub remote_control: RemoteControlReadyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOutput {
    pub pairing: RemoteControlPairingStartResponse,
    pub backend: BackendKind,
    pub socket_path: PathBuf,
    pub cli_version: String,
    pub app_server_version: String,
}

#[derive(Debug, Clone)]
pub struct DaemonPaths {
    pub code_home: PathBuf,
    pub state_dir: PathBuf,
    pub pid_file: PathBuf,
    pub process_lock_file: PathBuf,
    pub operation_lock_file: PathBuf,
    pub settings_file: PathBuf,
    pub installation_id_file: PathBuf,
    pub app_installation_id_file: PathBuf,
    pub stderr_log_file: PathBuf,
    pub socket_path: PathBuf,
    pub executable: PathBuf,
}

impl DaemonPaths {
    pub fn new(code_home: PathBuf, executable: PathBuf) -> Self {
        let state_dir = code_home.join("app-server-daemon");
        Self {
            pid_file: state_dir.join("app-server.pid"),
            process_lock_file: state_dir.join("app-server.pid.lock"),
            operation_lock_file: state_dir.join("daemon.lock"),
            settings_file: state_dir.join("settings.json"),
            installation_id_file: state_dir.join("installation_id"),
            app_installation_id_file: code_home.join("installation_id"),
            stderr_log_file: state_dir.join("app-server.stderr.log"),
            socket_path: state_dir.join("app-server.sock"),
            code_home,
            state_dir,
            executable,
        }
    }
}

#[async_trait::async_trait]
pub trait ControlClient: Send + Sync {
    async fn probe(&self, socket_path: &Path) -> Result<ProbeInfo>;
    async fn enable(&self, socket_path: &Path) -> Result<RemoteControlReadyStatus>;
    async fn disable(&self, socket_path: &Path) -> Result<RemoteControlReadyStatus>;
    async fn start_pairing(
        &self,
        socket_path: &Path,
    ) -> Result<RemoteControlPairingStartResponse>;
}

pub struct Daemon {
    paths: DaemonPaths,
    backend: Arc<dyn ProcessBackend>,
    control_client: Arc<dyn ControlClient>,
    startup_timeout: Duration,
    poll_interval: Duration,
}

impl Daemon {
    pub fn new(paths: DaemonPaths) -> Self {
        let backend = Arc::new(PidBackend::new(paths.clone()));
        Self {
            paths,
            backend,
            control_client: Arc::new(ControlSocketClient),
            startup_timeout: STARTUP_TIMEOUT,
            poll_interval: STARTUP_POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_components(
        paths: DaemonPaths,
        backend: Arc<dyn ProcessBackend>,
        control_client: Arc<dyn ControlClient>,
        startup_timeout: Duration,
        poll_interval: Duration,
    ) -> Self {
        Self {
            paths,
            backend,
            control_client,
            startup_timeout,
            poll_interval,
        }
    }

    pub async fn run(&self, command: LifecycleCommand) -> Result<LifecycleOutput> {
        self.prepare_operation_lock().await?;
        let _operation_lock = self.acquire_operation_lock().await?;
        self.prepare_locked_state().await?;
        match command {
            LifecycleCommand::Start => self.start_locked(LifecycleStatus::Started).await,
            LifecycleCommand::Restart => self.restart_locked().await,
            LifecycleCommand::Stop => self.stop_locked().await,
            LifecycleCommand::Version => self.version_locked().await,
        }
    }

    pub async fn set_remote_control(
        &self,
        mode: RemoteControlMode,
    ) -> Result<RemoteControlOutput> {
        self.prepare_operation_lock().await?;
        let _operation_lock = self.acquire_operation_lock().await?;
        self.prepare_locked_state().await?;
        let previous = DaemonSettings::load(&self.paths.settings_file).await?;
        let lifecycle = self.ensure_running_locked().await?;
        let ready = match mode {
            RemoteControlMode::Enabled => {
                self.control_client.enable(&self.paths.socket_path).await?
            }
            RemoteControlMode::Disabled => {
                self.control_client.disable(&self.paths.socket_path).await?
            }
        };
        let remote_control_enabled = mode == RemoteControlMode::Enabled;
        DaemonSettings {
            remote_control_enabled,
        }
        .save(&self.paths.settings_file)
        .await?;
        let status = match (mode, previous.remote_control_enabled) {
            (RemoteControlMode::Enabled, true) => RemoteControlStatus::AlreadyEnabled,
            (RemoteControlMode::Enabled, false) => RemoteControlStatus::Enabled,
            (RemoteControlMode::Disabled, false) => RemoteControlStatus::AlreadyDisabled,
            (RemoteControlMode::Disabled, true) => RemoteControlStatus::Disabled,
        };
        Ok(RemoteControlOutput {
            status,
            backend: BackendKind::Pid,
            remote_control_enabled,
            socket_path: self.paths.socket_path.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            app_server_version: lifecycle
                .app_server_version
                .ok_or_else(|| anyhow!("running app-server did not report a version"))?,
            ready,
        })
    }

    pub async fn ensure_remote_control_ready(&self) -> Result<RemoteControlReadyOutput> {
        self.prepare_operation_lock().await?;
        let _operation_lock = self.acquire_operation_lock().await?;
        self.prepare_locked_state().await?;
        DaemonSettings {
            remote_control_enabled: true,
        }
        .save(&self.paths.settings_file)
        .await?;
        let daemon = self.ensure_running_locked().await?;
        let remote_control = self.control_client.enable(&self.paths.socket_path).await?;
        Ok(RemoteControlReadyOutput {
            daemon,
            remote_control,
        })
    }

    pub async fn start_pairing(&self) -> Result<PairingOutput> {
        self.prepare_operation_lock().await?;
        let _operation_lock = self.acquire_operation_lock().await?;
        self.prepare_locked_state().await?;
        let lifecycle = self.ensure_running_locked().await?;
        let pairing = self
            .control_client
            .start_pairing(&self.paths.socket_path)
            .await?;
        Ok(PairingOutput {
            pairing,
            backend: BackendKind::Pid,
            socket_path: self.paths.socket_path.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            app_server_version: lifecycle
                .app_server_version
                .ok_or_else(|| anyhow!("running app-server did not report a version"))?,
        })
    }

    async fn ensure_running_locked(&self) -> Result<LifecycleOutput> {
        match self.backend.state().await? {
            BackendState::Running(identity) => {
                let probe = self.wait_until_ready().await?;
                Ok(self.output(
                    LifecycleStatus::AlreadyRunning,
                    Some(BackendKind::Pid),
                    Some(identity.pid),
                    Some(probe.app_server_version),
                ))
            }
            BackendState::NotRunning => self.start_process_locked(LifecycleStatus::Started).await,
            BackendState::Stale(_) => {
                self.cleanup_stale_state().await?;
                self.start_process_locked(LifecycleStatus::Started).await
            }
        }
    }

    async fn start_locked(&self, started_status: LifecycleStatus) -> Result<LifecycleOutput> {
        match self.backend.state().await? {
            BackendState::Running(identity) => {
                let probe = self.wait_until_ready().await?;
                Ok(self.output(
                    LifecycleStatus::AlreadyRunning,
                    Some(BackendKind::Pid),
                    Some(identity.pid),
                    Some(probe.app_server_version),
                ))
            }
            BackendState::Stale(_) => {
                self.cleanup_stale_state().await?;
                self.start_process_locked(started_status).await
            }
            BackendState::NotRunning => {
                self.cleanup_stale_socket().await?;
                self.start_process_locked(started_status).await
            }
        }
    }

    async fn start_process_locked(&self, status: LifecycleStatus) -> Result<LifecycleOutput> {
        let settings = DaemonSettings::load(&self.paths.settings_file).await?;
        let identity = self
            .backend
            .start(StartRequest::new(
                &self.paths,
                settings.remote_control_enabled,
            ))
            .await?;
        let probe = match self.wait_until_ready().await {
            Ok(probe) => probe,
            Err(err) => {
                return Err(self.failed_start_error(&identity, err).await);
            }
        };
        let remote_result = if settings.remote_control_enabled {
            self.control_client.enable(&self.paths.socket_path).await
        } else {
            self.control_client.disable(&self.paths.socket_path).await
        };
        if let Err(err) = remote_result {
            let err = err.context("failed to apply daemon remote-control setting");
            return Err(self.failed_start_error(&identity, err).await);
        }
        Ok(self.output(
            status,
            Some(BackendKind::Pid),
            Some(identity.pid),
            Some(probe.app_server_version),
        ))
    }

    async fn restart_locked(&self) -> Result<LifecycleOutput> {
        match self.backend.state().await? {
            BackendState::Running(identity) => self.backend.stop(&identity).await?,
            BackendState::Stale(_) | BackendState::NotRunning => {}
        }
        self.cleanup_stale_state().await?;
        self.start_process_locked(LifecycleStatus::Restarted).await
    }

    async fn failed_start_error(
        &self,
        identity: &ProcessIdentity,
        original: anyhow::Error,
    ) -> anyhow::Error {
        if let Err(cleanup) = self.backend.stop(identity).await {
            return original.context(format!(
                "additionally failed to stop app-server process {}: {cleanup:#}",
                identity.pid
            ));
        }
        if let Err(cleanup) = self.cleanup_stale_socket().await {
            return original.context(format!(
                "additionally failed to remove stale control socket: {cleanup:#}"
            ));
        }
        original
    }

    async fn stop_locked(&self) -> Result<LifecycleOutput> {
        match self.backend.state().await? {
            BackendState::Running(identity) => {
                self.backend.stop(&identity).await?;
                self.cleanup_stale_state().await?;
                Ok(self.output(
                    LifecycleStatus::Stopped,
                    Some(BackendKind::Pid),
                    None,
                    None,
                ))
            }
            BackendState::Stale(_) => {
                self.cleanup_stale_state().await?;
                Ok(self.output(LifecycleStatus::NotRunning, None, None, None))
            }
            BackendState::NotRunning => {
                self.cleanup_stale_socket().await?;
                Ok(self.output(LifecycleStatus::NotRunning, None, None, None))
            }
        }
    }

    async fn version_locked(&self) -> Result<LifecycleOutput> {
        match self.backend.state().await? {
            BackendState::Running(identity) => {
                let probe = self.control_client.probe(&self.paths.socket_path).await?;
                Ok(self.output(
                    LifecycleStatus::Running,
                    Some(BackendKind::Pid),
                    Some(identity.pid),
                    Some(probe.app_server_version),
                ))
            }
            BackendState::Stale(_) => {
                self.cleanup_stale_state().await?;
                Ok(self.output(LifecycleStatus::NotRunning, None, None, None))
            }
            BackendState::NotRunning => {
                Ok(self.output(LifecycleStatus::NotRunning, None, None, None))
            }
        }
    }

    async fn wait_until_ready(&self) -> Result<ProbeInfo> {
        let deadline = Instant::now() + self.startup_timeout;
        loop {
            match self.control_client.probe(&self.paths.socket_path).await {
                Ok(probe) => return Ok(probe),
                Err(_) if Instant::now() < deadline => sleep(self.poll_interval).await,
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "timed out waiting for app-server control socket {}",
                            self.paths.socket_path.display()
                        )
                    });
                }
            }
        }
    }

    async fn prepare_operation_lock(&self) -> Result<()> {
        process::prepare_private_state_dir(&self.paths.state_dir).await?;
        process::ensure_private_file(&self.paths.operation_lock_file, b"").await
    }

    async fn prepare_locked_state(&self) -> Result<()> {
        if !self.paths.settings_file.exists() {
            DaemonSettings::default()
                .save(&self.paths.settings_file)
                .await?;
        } else {
            process::validate_owned_path(
                &self.paths.settings_file,
                unsafe { libc::geteuid() },
            )
            .await?;
            process::make_private(&self.paths.settings_file, 0o600).await?;
        }
        process::ensure_stable_installation_id(
            &self.paths.installation_id_file,
            &self.paths.app_installation_id_file,
        )
        .await?;
        Ok(())
    }

    async fn acquire_operation_lock(&self) -> Result<tokio::fs::File> {
        use std::os::fd::AsRawFd;

        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.paths.operation_lock_file)
            .await
            .with_context(|| {
                format!(
                    "failed to open daemon operation lock {}",
                    self.paths.operation_lock_file.display()
                )
            })?;
        let deadline = Instant::now() + OPERATION_LOCK_TIMEOUT;
        loop {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(file);
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(err).context("failed to lock daemon operation");
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for daemon operation lock {}",
                    self.paths.operation_lock_file.display()
                ));
            }
            sleep(self.poll_interval).await;
        }
    }

    async fn cleanup_stale_state(&self) -> Result<()> {
        process::remove_owned_path_if_present(&self.paths.pid_file).await?;
        self.cleanup_stale_socket().await
    }

    async fn cleanup_stale_socket(&self) -> Result<()> {
        if process::owned_unix_socket_is_live(&self.paths.socket_path).await? {
            return Err(anyhow!(
                "app-server control socket {} is live but is not managed by this daemon",
                self.paths.socket_path.display()
            ));
        }
        process::remove_owned_path_if_present(&self.paths.socket_path).await
    }

    fn output(
        &self,
        status: LifecycleStatus,
        backend: Option<BackendKind>,
        pid: Option<u32>,
        app_server_version: Option<String>,
    ) -> LifecycleOutput {
        LifecycleOutput {
            status,
            backend,
            pid,
            socket_path: self.paths.socket_path.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            app_server_version,
        }
    }
}

pub async fn run(command: LifecycleCommand) -> Result<LifecycleOutput> {
    default_daemon()?.run(command).await
}

pub async fn set_remote_control(mode: RemoteControlMode) -> Result<RemoteControlOutput> {
    default_daemon()?.set_remote_control(mode).await
}

pub async fn ensure_remote_control_ready() -> Result<RemoteControlReadyOutput> {
    default_daemon()?.ensure_remote_control_ready().await
}

pub async fn start_remote_control_pairing() -> Result<RemoteControlPairingStartResponse> {
    Ok(default_daemon()?.start_pairing().await?.pairing)
}

pub async fn probe_app_server_version(socket_path: &Path) -> Result<String> {
    Ok(client::probe(socket_path).await?.app_server_version)
}

pub async fn enable_remote_control_on_socket(
    socket_path: &Path,
    connect_timeout: Duration,
    connect_retry_delay: Duration,
) -> Result<RemoteControlReadyStatus> {
    remote_control_client::enable_with_connect_retry(
        socket_path,
        connect_timeout,
        connect_retry_delay,
    )
    .await
}

fn default_daemon() -> Result<Daemon> {
    let code_home = if let Some(path) = env::var_os("CODE_HOME") {
        PathBuf::from(path)
    } else if let Some(path) = env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else {
        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        PathBuf::from(home).join(".code")
    };
    let executable = resolve_app_server_executable()?;
    Ok(Daemon::new(DaemonPaths::new(code_home, executable)))
}

fn resolve_app_server_executable() -> Result<PathBuf> {
    let current = env::current_exe().context("failed to locate current code executable")?;
    if process::is_standalone_app_server_executable(&current) {
        return Ok(current);
    }
    if let Some(parent) = current.parent() {
        for name in ["code-app-server", "codex-app-server"] {
            let candidate = parent.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests;
