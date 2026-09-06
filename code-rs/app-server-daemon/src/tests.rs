use std::collections::VecDeque;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use pretty_assertions::assert_eq;
use tokio::sync::Mutex;

use super::BackendState;
use super::ControlClient;
use super::Daemon;
use super::DaemonPaths;
use super::LifecycleCommand;
use super::LifecycleStatus;
use super::PairingOutput;
use super::ProbeInfo;
use super::ProcessBackend;
use super::ProcessIdentity;
use super::RemoteControlMode;
use super::RemoteControlReadyStatus;
use super::RemoteControlStatus;
use super::StartRequest;
use super::validate_owned_path;

#[derive(Default)]
struct FakeBackend {
    states: Mutex<VecDeque<BackendState>>,
    starts: Mutex<Vec<StartRequest>>,
    stops: Mutex<Vec<ProcessIdentity>>,
}

impl FakeBackend {
    fn with_states(states: impl IntoIterator<Item = BackendState>) -> Self {
        Self {
            states: Mutex::new(states.into_iter().collect()),
            ..Self::default()
        }
    }
}

#[async_trait]
impl ProcessBackend for FakeBackend {
    async fn state(&self) -> Result<BackendState> {
        Ok(self
            .states
            .lock()
            .await
            .pop_front()
            .unwrap_or(BackendState::NotRunning))
    }

    async fn start(&self, request: StartRequest) -> Result<ProcessIdentity> {
        self.starts.lock().await.push(request);
        Ok(ProcessIdentity {
            pid: 4242,
            start_time: "fake-start".to_string(),
        })
    }

    async fn stop(&self, identity: &ProcessIdentity) -> Result<()> {
        self.stops.lock().await.push(identity.clone());
        Ok(())
    }
}

struct FakeControlClient {
    probes: Mutex<VecDeque<Result<ProbeInfo>>>,
    enabled: Mutex<usize>,
    disabled: Mutex<usize>,
    pairings: Mutex<usize>,
}

impl FakeControlClient {
    fn ready(version: &str) -> Self {
        Self {
            probes: Mutex::new(VecDeque::from([Ok(ProbeInfo {
                app_server_version: version.to_string(),
            })])),
            enabled: Mutex::new(0),
            disabled: Mutex::new(0),
            pairings: Mutex::new(0),
        }
    }

    fn with_probes(probes: impl IntoIterator<Item = Result<ProbeInfo>>) -> Self {
        Self {
            probes: Mutex::new(probes.into_iter().collect()),
            enabled: Mutex::new(0),
            disabled: Mutex::new(0),
            pairings: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ControlClient for FakeControlClient {
    async fn probe(&self, _socket_path: &std::path::Path) -> Result<ProbeInfo> {
        self.probes
            .lock()
            .await
            .pop_front()
            .unwrap_or_else(|| anyhow::bail!("not ready"))
    }

    async fn enable(&self, _socket_path: &std::path::Path) -> Result<RemoteControlReadyStatus> {
        *self.enabled.lock().await += 1;
        Ok(ready_status(RemoteControlConnectionStatus::Connected))
    }

    async fn disable(&self, _socket_path: &std::path::Path) -> Result<RemoteControlReadyStatus> {
        *self.disabled.lock().await += 1;
        Ok(ready_status(RemoteControlConnectionStatus::Disabled))
    }

    async fn start_pairing(
        &self,
        _socket_path: &std::path::Path,
    ) -> Result<RemoteControlPairingStartResponse> {
        *self.pairings.lock().await += 1;
        Ok(RemoteControlPairingStartResponse {
            pairing_code: "pair-code".to_string(),
            manual_pairing_code: Some("manual-code".to_string()),
            environment_id: "test-environment".to_string(),
            expires_at: 1234,
        })
    }
}

fn ready_status(status: RemoteControlConnectionStatus) -> RemoteControlReadyStatus {
    RemoteControlReadyStatus {
        status,
        server_name: "test-server".to_string(),
        installation_id: "test-installation".to_string(),
        environment_id: Some("test-environment".to_string()),
    }
}

fn daemon(
    temp: &tempfile::TempDir,
    backend: Arc<FakeBackend>,
    client: Arc<FakeControlClient>,
) -> Daemon {
    Daemon::with_components(
        DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code")),
        backend,
        client,
        Duration::from_millis(30),
        Duration::from_millis(1),
    )
}

fn identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 1234,
        start_time: "known-start".to_string(),
    }
}

#[tokio::test]
async fn lifecycle_covers_start_already_running_restart_stop_and_version() {
    let temp = tempfile::tempdir().expect("temp code home");
    let backend = Arc::new(FakeBackend::with_states([
        BackendState::NotRunning,
        BackendState::Running(identity()),
        BackendState::Running(identity()),
        BackendState::Running(identity()),
        BackendState::NotRunning,
        BackendState::Running(identity()),
    ]));
    let client = Arc::new(FakeControlClient::with_probes([
        Err(anyhow::anyhow!("starting")),
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.1.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.1.0".to_string(),
        }),
    ]));
    let daemon = daemon(&temp, backend.clone(), client);

    assert_eq!(
        daemon.run(LifecycleCommand::Start).await.expect("start").status,
        LifecycleStatus::Started
    );
    assert_eq!(
        daemon
            .run(LifecycleCommand::Start)
            .await
            .expect("already running")
            .status,
        LifecycleStatus::AlreadyRunning
    );
    assert_eq!(
        daemon
            .run(LifecycleCommand::Restart)
            .await
            .expect("restart")
            .status,
        LifecycleStatus::Restarted
    );
    assert_eq!(
        daemon.run(LifecycleCommand::Stop).await.expect("stop").status,
        LifecycleStatus::Stopped
    );
    assert_eq!(
        daemon
            .run(LifecycleCommand::Stop)
            .await
            .expect("already stopped")
            .status,
        LifecycleStatus::NotRunning
    );
    let version = daemon
        .run(LifecycleCommand::Version)
        .await
        .expect("version");
    assert_eq!(version.status, LifecycleStatus::Running);
    assert_eq!(version.app_server_version.as_deref(), Some("1.1.0"));

    assert_eq!(backend.starts.lock().await.len(), 2);
    assert_eq!(backend.stops.lock().await.len(), 2);
    assert!(backend.starts.lock().await.iter().all(|request| {
        request.socket_path.is_absolute()
            && request.listen_url.starts_with("unix:///")
            && !request.listen_url.contains("127.0.0.1")
    }));
}

#[tokio::test]
async fn readiness_timeout_stops_the_spawned_process() {
    let temp = tempfile::tempdir().expect("temp code home");
    let backend = Arc::new(FakeBackend::with_states([BackendState::NotRunning]));
    let client = Arc::new(FakeControlClient::with_probes([]));
    let daemon = daemon(&temp, backend.clone(), client);

    let error = daemon
        .run(LifecycleCommand::Start)
        .await
        .expect_err("startup must time out");
    assert!(error.to_string().contains("timed out waiting for app-server"));
    assert_eq!(backend.stops.lock().await.len(), 1);
}

#[tokio::test]
async fn version_reports_not_running_without_starting_a_process() {
    let temp = tempfile::tempdir().expect("temp code home");
    let backend = Arc::new(FakeBackend::with_states([BackendState::NotRunning]));
    let client = Arc::new(FakeControlClient::ready("unused"));
    let daemon = daemon(&temp, backend.clone(), client);

    let output = daemon
        .run(LifecycleCommand::Version)
        .await
        .expect("version while stopped");
    assert_eq!(output.status, LifecycleStatus::NotRunning);
    assert_eq!(output.backend, None);
    assert_eq!(output.pid, None);
    assert_eq!(output.app_server_version, None);
    assert!(backend.starts.lock().await.is_empty());
}

#[tokio::test]
async fn stale_owned_pid_and_socket_are_removed_before_start() {
    let temp = tempfile::tempdir().expect("temp code home");
    let paths = DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code"));
    tokio::fs::create_dir_all(&paths.state_dir)
        .await
        .expect("state dir");
    tokio::fs::write(&paths.pid_file, b"stale")
        .await
        .expect("pid file");
    tokio::fs::write(&paths.socket_path, b"stale")
        .await
        .expect("socket path");

    let backend = Arc::new(FakeBackend::with_states([BackendState::Stale(identity())]));
    let client = Arc::new(FakeControlClient::ready("1.0.0"));
    let daemon = Daemon::with_components(
        paths.clone(),
        backend,
        client,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );

    daemon.run(LifecycleCommand::Start).await.expect("start");
    assert!(!paths.pid_file.exists());
    assert!(!paths.socket_path.exists());
}

#[tokio::test]
async fn state_paths_and_files_are_stable_and_private() {
    let temp = tempfile::tempdir().expect("temp code home");
    let paths = DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code"));
    let backend = Arc::new(FakeBackend::with_states([BackendState::Running(identity())]));
    let client = Arc::new(FakeControlClient::ready("1.0.0"));
    let daemon = Daemon::with_components(
        paths.clone(),
        backend,
        client,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );

    daemon
        .run(LifecycleCommand::Version)
        .await
        .expect("prepare state");
    let first = tokio::fs::read_to_string(&paths.installation_id_file)
        .await
        .expect("installation id");
    daemon
        .run(LifecycleCommand::Version)
        .await
        .expect("prepare state again");
    let second = tokio::fs::read_to_string(&paths.installation_id_file)
        .await
        .expect("installation id again");

    assert_eq!(paths.state_dir, temp.path().join("app-server-daemon"));
    assert_eq!(first, second);
    assert_eq!(
        tokio::fs::metadata(&paths.state_dir)
            .await
            .expect("state metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for path in [
        &paths.operation_lock_file,
        &paths.settings_file,
        &paths.installation_id_file,
    ] {
        assert_eq!(
            tokio::fs::metadata(path)
                .await
                .expect("private file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn ownership_validation_rejects_foreign_or_symlinked_state() {
    let temp = tempfile::tempdir().expect("temp code home");
    let path = temp.path().join("state");
    tokio::fs::write(&path, b"state").await.expect("state file");
    let uid = tokio::fs::symlink_metadata(&path)
        .await
        .expect("metadata")
        .uid();

    assert!(validate_owned_path(&path, uid).await.is_ok());
    assert!(validate_owned_path(&path, uid.saturating_add(1)).await.is_err());

    let link = temp.path().join("state-link");
    std::os::unix::fs::symlink(&path, &link).expect("symlink");
    assert!(validate_owned_path(&link, uid).await.is_err());
}

#[tokio::test]
async fn daemon_refuses_a_symlinked_operation_lock_without_touching_its_target() {
    let temp = tempfile::tempdir().expect("temp code home");
    let paths = DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code"));
    tokio::fs::create_dir_all(&paths.state_dir)
        .await
        .expect("state dir");
    let victim = temp.path().join("victim");
    tokio::fs::write(&victim, b"do not touch")
        .await
        .expect("victim");
    std::os::unix::fs::symlink(&victim, &paths.operation_lock_file).expect("lock symlink");

    let backend = Arc::new(FakeBackend::with_states([BackendState::NotRunning]));
    let client = Arc::new(FakeControlClient::ready("1.0.0"));
    let daemon = Daemon::with_components(
        paths,
        backend,
        client,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );

    assert!(daemon.run(LifecycleCommand::Version).await.is_err());
    assert_eq!(
        tokio::fs::read(&victim).await.expect("read victim"),
        b"do not touch"
    );
}

#[tokio::test]
async fn start_refuses_to_unlink_a_live_unmanaged_socket() {
    let temp = tempfile::tempdir().expect("temp code home");
    let paths = DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code"));
    tokio::fs::create_dir_all(&paths.state_dir)
        .await
        .expect("state dir");
    let listener = tokio::net::UnixListener::bind(&paths.socket_path).expect("live socket");
    tokio::fs::set_permissions(
        &paths.socket_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .await
    .expect("private socket");
    let accept = tokio::spawn(async move {
        let _ = listener.accept().await;
    });
    let backend = Arc::new(FakeBackend::with_states([BackendState::NotRunning]));
    let client = Arc::new(FakeControlClient::ready("1.0.0"));
    let daemon = Daemon::with_components(
        paths.clone(),
        backend.clone(),
        client,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );

    let error = daemon
        .run(LifecycleCommand::Start)
        .await
        .expect_err("live unmanaged socket must be preserved");
    assert!(error.to_string().contains("live but is not managed"));
    assert!(paths.socket_path.exists());
    assert!(backend.starts.lock().await.is_empty());
    accept.await.expect("accept task");
}

#[tokio::test]
async fn remote_control_enable_disable_and_pairing_use_initialized_control_client() {
    let temp = tempfile::tempdir().expect("temp code home");
    let backend = Arc::new(FakeBackend::with_states([
        BackendState::Running(identity()),
        BackendState::Running(identity()),
        BackendState::Running(identity()),
        BackendState::Running(identity()),
    ]));
    let client = Arc::new(FakeControlClient::with_probes([
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
        Ok(ProbeInfo {
            app_server_version: "1.0.0".to_string(),
        }),
    ]));
    let daemon = daemon(&temp, backend, client.clone());

    let enabled = daemon
        .set_remote_control(RemoteControlMode::Enabled)
        .await
        .expect("enable");
    assert_eq!(enabled.status, RemoteControlStatus::Enabled);
    let disabled = daemon
        .set_remote_control(RemoteControlMode::Disabled)
        .await
        .expect("disable");
    assert_eq!(disabled.status, RemoteControlStatus::Disabled);
    let PairingOutput { pairing, .. } = daemon.start_pairing().await.expect("pairing");
    assert_eq!(pairing.manual_pairing_code.as_deref(), Some("manual-code"));
    let ready = daemon
        .ensure_remote_control_ready()
        .await
        .expect("ensure ready");
    assert_eq!(
        ready.remote_control.status,
        RemoteControlConnectionStatus::Connected
    );

    assert_eq!(*client.enabled.lock().await, 2);
    assert_eq!(*client.disabled.lock().await, 1);
    assert_eq!(*client.pairings.lock().await, 1);
}
