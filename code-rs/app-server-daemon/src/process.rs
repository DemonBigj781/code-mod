use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs;
use tokio::process::Command;
use tokio::time::Instant;
use tokio::time::sleep;
use uuid::Uuid;

use crate::BackendState;
use crate::DaemonPaths;
use crate::ProcessBackend;
use crate::StartRequest;

const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(10);
const FORCE_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: String,
}

pub struct PidBackend {
    paths: DaemonPaths,
}

impl PidBackend {
    pub fn new(paths: DaemonPaths) -> Self {
        Self { paths }
    }
}

#[async_trait]
impl ProcessBackend for PidBackend {
    async fn state(&self) -> Result<BackendState> {
        match fs::symlink_metadata(&self.paths.pid_file).await {
            Ok(_) => validate_owned_path(&self.paths.pid_file, current_uid()).await?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BackendState::NotRunning);
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to inspect pid file {}", self.paths.pid_file.display())
                });
            }
        }
        let contents = match fs::read_to_string(&self.paths.pid_file).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BackendState::NotRunning);
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to read pid file {}", self.paths.pid_file.display())
                });
            }
        };
        let identity: ProcessIdentity = serde_json::from_str(&contents)
            .with_context(|| format!("invalid pid file {}", self.paths.pid_file.display()))?;
        if process_matches(&identity).await? {
            Ok(BackendState::Running(identity))
        } else {
            Ok(BackendState::Stale(identity))
        }
    }

    async fn start(&self, request: StartRequest) -> Result<ProcessIdentity> {
        ensure_private_file(&self.paths.process_lock_file, b"").await?;
        let _reservation = lock_file(&self.paths.process_lock_file).await?;
        if self.paths.pid_file.exists() {
            bail!("pid file already exists: {}", self.paths.pid_file.display());
        }

        let stderr = open_private_log(&request.stderr_log_file).await?;
        let mut command = Command::new(&request.executable);
        if !is_standalone_app_server_executable(&request.executable) {
            command.arg("app-server");
        }
        command
            .arg("--listen")
            .arg(&request.listen_url)
            .env("CODE_HOME", &request.code_home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr.into_std().await));
        if request.remote_control_enabled {
            command.arg("--remote-control");
        }
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().with_context(|| {
            format!(
                "failed to spawn detached app-server using {}",
                request.executable.display()
            )
        })?;
        let pid = child.id().context("spawned app-server has no pid")?;
        let start_time = match read_process_start_time(pid).await {
            Ok(start_time) => start_time,
            Err(err) => {
                let _ = signal_process(pid, libc::SIGTERM);
                return Err(err).context("failed to record app-server process identity");
            }
        };
        let identity = ProcessIdentity { pid, start_time };
        if let Err(err) = write_pid_file(&self.paths.pid_file, &identity).await {
            let _ = signal_process(pid, libc::SIGTERM);
            return Err(err).context("failed to persist app-server process identity");
        }
        Ok(identity)
    }

    async fn stop(&self, identity: &ProcessIdentity) -> Result<()> {
        if !process_matches(identity).await? {
            remove_owned_path_if_present(&self.paths.pid_file).await?;
            return Ok(());
        }
        validate_process_owner(identity.pid)?;
        signal_process(identity.pid, libc::SIGTERM)?;
        if wait_for_exit(identity, STOP_GRACE_PERIOD).await? {
            remove_owned_path_if_present(&self.paths.pid_file).await?;
            return Ok(());
        }
        signal_process(identity.pid, libc::SIGKILL)?;
        if !wait_for_exit(identity, FORCE_STOP_TIMEOUT).await? {
            bail!("timed out stopping app-server process {}", identity.pid);
        }
        remove_owned_path_if_present(&self.paths.pid_file).await
    }
}

pub(crate) fn is_standalone_app_server_executable(path: &Path) -> bool {
    matches!(
        path.file_stem().and_then(|name| name.to_str()),
        Some("code-app-server" | "codex-app-server")
    )
}

pub async fn prepare_private_state_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("daemon state path is not a directory: {}", path.display());
            }
            if metadata.uid() != current_uid() {
                bail!("daemon state directory is not owned by the current user");
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(path).await.with_context(|| {
                format!("failed to create daemon state directory {}", path.display())
            })?;
        }
        Err(err) => return Err(err).context("failed to inspect daemon state directory"),
    }
    make_private(path, 0o700).await
}

pub async fn ensure_private_file(path: &Path, default_contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        prepare_private_state_dir(parent).await?;
    }
    match fs::symlink_metadata(path).await {
        Ok(_) => validate_owned_path(path, current_uid()).await?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
    let mut options = fs::OpenOptions::new();
    options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .await
        .with_context(|| format!("failed to create private file {}", path.display()))?;
    if file.metadata().await?.len() == 0 && !default_contents.is_empty() {
        use tokio::io::AsyncWriteExt;
        file.write_all(default_contents).await?;
        file.sync_all().await?;
    }
    validate_owned_path(path, current_uid()).await?;
    make_private(path, 0o600).await
}

pub async fn make_private(path: &Path, mode: u32) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("refusing to change permissions through symlink {}", path.display());
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
        .await
        .with_context(|| format!("failed to set private permissions on {}", path.display()))
}

pub async fn validate_owned_path(path: &Path, expected_uid: u32) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("refusing symlinked daemon state path {}", path.display());
    }
    if metadata.uid() != expected_uid {
        bail!("daemon state path is not owned by the expected user: {}", path.display());
    }
    Ok(())
}

pub async fn remove_owned_path_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path).await {
        Ok(_) => validate_owned_path(path, current_uid()).await?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context("failed to inspect stale daemon state"),
    }
    fs::remove_file(path)
        .await
        .with_context(|| format!("failed to remove stale daemon state {}", path.display()))
}

pub async fn owned_unix_socket_is_live(path: &Path) -> Result<bool> {
    use std::os::unix::fs::FileTypeExt;

    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    };
    validate_owned_path(path, current_uid()).await?;
    if !metadata.file_type().is_socket() {
        return Ok(false);
    }
    match tokio::time::timeout(
        Duration::from_millis(250),
        tokio::net::UnixStream::connect(path),
    )
    .await
    {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(err))
            if matches!(
                err.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Ok(Err(err)) => Err(err).with_context(|| {
            format!("failed to test app-server control socket {}", path.display())
        }),
        Err(_) => Ok(true),
    }
}

pub async fn ensure_stable_installation_id(state_path: &Path, app_path: &Path) -> Result<String> {
    let existing_app = read_valid_uuid(app_path).await?;
    let existing_state = read_valid_uuid(state_path).await?;
    let installation_id = existing_app
        .clone()
        .or_else(|| existing_state.clone())
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    if existing_state.as_deref() != Some(installation_id.as_str()) {
        write_private_contents(state_path, installation_id.as_bytes()).await?;
    } else {
        make_private(state_path, 0o600).await?;
    }
    if existing_app.as_deref() != Some(installation_id.as_str()) {
        write_private_contents(app_path, installation_id.as_bytes()).await?;
    } else {
        make_private(app_path, 0o600).await?;
    }
    Ok(installation_id)
}

async fn read_valid_uuid(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path).await {
        Ok(_) => validate_owned_path(path, current_uid()).await?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
    let contents = match fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    Ok(Uuid::parse_str(contents.trim())
        .ok()
        .map(|value| value.to_string()))
}

pub(crate) async fn write_private_contents(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    match fs::symlink_metadata(path).await {
        Ok(_) => validate_owned_path(path, current_uid()).await?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::now_v7()));
    let mut options = fs::OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(&temporary).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(contents).await?;
    file.sync_all().await?;
    drop(file);
    if let Err(err) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(err).with_context(|| format!("failed to install {}", path.display()));
    }
    make_private(path, 0o600).await
}

async fn open_private_log(path: &Path) -> Result<fs::File> {
    match fs::symlink_metadata(path).await {
        Ok(_) => validate_owned_path(path, current_uid()).await?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
    let mut options = fs::OpenOptions::new();
    options
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .await
        .with_context(|| format!("failed to open app-server log {}", path.display()))?;
    make_private(path, 0o600).await?;
    Ok(file)
}

async fn write_pid_file(path: &Path, identity: &ProcessIdentity) -> Result<()> {
    let contents = serde_json::to_vec(identity).context("failed to serialize pid record")?;
    write_private_contents(path, &contents).await
}

async fn lock_file(path: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;

    let file = fs::OpenOptions::new().read(true).write(true).open(path).await?;
    let deadline = Instant::now() + PROCESS_LOCK_TIMEOUT;
    loop {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(file);
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(err).context("failed to lock pid reservation");
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for pid reservation lock {}", path.display());
        }
        sleep(STOP_POLL_INTERVAL).await;
    }
}

async fn process_matches(identity: &ProcessIdentity) -> Result<bool> {
    if !process_exists(identity.pid) {
        return Ok(false);
    }
    match read_process_start_time(identity.pid).await {
        Ok(start_time) => Ok(start_time == identity.start_time),
        Err(_) if !process_exists(identity.pid) => Ok(false),
        Err(err) => Err(err),
    }
}

async fn read_process_start_time(pid: u32) -> Result<String> {
    let path = format!("/proc/{pid}/stat");
    let contents = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read process identity from {path}"))?;
    let end = contents
        .rfind(')')
        .ok_or_else(|| anyhow!("invalid process stat for pid {pid}"))?;
    contents[end + 1..]
        .split_whitespace()
        .nth(19)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("process stat omitted start time for pid {pid}"))
}

fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn validate_process_owner(pid: u32) -> Result<()> {
    let metadata = std::fs::metadata(format!("/proc/{pid}"))
        .with_context(|| format!("failed to inspect app-server process {pid}"))?;
    if metadata.uid() != current_uid() {
        bail!("refusing to signal app-server process {pid} owned by another user");
    }
    Ok(())
}

fn signal_process(pid: u32, signal: i32) -> Result<()> {
    let raw_pid = libc::pid_t::try_from(pid).context("app-server pid is out of range")?;
    let result = unsafe { libc::kill(raw_pid, signal) };
    if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to signal app-server process {pid}"))
    }
}

async fn wait_for_exit(identity: &ProcessIdentity, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_matches(identity).await? {
            return Ok(true);
        }
        sleep(STOP_POLL_INTERVAL).await;
    }
    Ok(!process_matches(identity).await?)
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Duration;

    use anyhow::Result;
    use pretty_assertions::assert_eq;

    use super::PidBackend;
    use super::ProcessIdentity;
    use super::is_standalone_app_server_executable;
    use super::read_process_start_time;
    use crate::BackendState;
    use crate::DaemonPaths;
    use crate::ProcessBackend;
    use crate::StartRequest;

    #[tokio::test]
    async fn pid_state_uses_start_time_to_reject_pid_reuse() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::new(temp.path().to_path_buf(), PathBuf::from("/bin/code"));
        super::prepare_private_state_dir(&paths.state_dir).await?;
        let pid = std::process::id();
        let start_time = read_process_start_time(pid).await?;
        super::write_private_contents(
            &paths.pid_file,
            &serde_json::to_vec(&ProcessIdentity {
                pid,
                start_time: start_time.clone(),
            })?,
        )
        .await?;
        let backend = PidBackend::new(paths.clone());
        assert_eq!(
            backend.state().await?,
            BackendState::Running(ProcessIdentity { pid, start_time })
        );

        super::write_private_contents(
            &paths.pid_file,
            &serde_json::to_vec(&ProcessIdentity {
                pid,
                start_time: "different-start-time".to_string(),
            })?,
        )
        .await?;
        assert!(matches!(backend.state().await?, BackendState::Stale(_)));
        Ok(())
    }

    #[test]
    fn recognizes_dedicated_app_server_binary_names() {
        assert!(is_standalone_app_server_executable(Path::new(
            "/tmp/code-app-server"
        )));
        assert!(is_standalone_app_server_executable(Path::new(
            "/tmp/codex-app-server"
        )));
        assert!(!is_standalone_app_server_executable(Path::new(
            "/tmp/code"
        )));
    }

    #[tokio::test]
    async fn pid_backend_spawns_unix_only_command_and_stops_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let executable = temp.path().join("fake-code");
        tokio::fs::write(
            &executable,
            b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CODE_HOME/args\"\ntrap 'exit 0' TERM INT\nwhile :; do sleep 0.05; done\n",
        )
        .await?;
        tokio::fs::set_permissions(&executable, PermissionsExt::from_mode(0o700)).await?;
        let paths = DaemonPaths::new(temp.path().to_path_buf(), executable);
        super::prepare_private_state_dir(&paths.state_dir).await?;
        let backend = PidBackend::new(paths.clone());
        let request = StartRequest::new(&paths, true);
        let identity = backend.start(request).await?;

        let args_path = temp.path().join("args");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !args_path.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let args = tokio::fs::read_to_string(&args_path).await?;
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "app-server",
                "--listen",
                &format!("unix://{}", paths.socket_path.display()),
                "--remote-control",
            ]
        );
        assert_eq!(
            tokio::fs::metadata(&paths.pid_file)
                .await?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            tokio::fs::metadata(&paths.stderr_log_file)
                .await?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        backend.stop(&identity).await?;
        assert!(!paths.pid_file.exists());
        Ok(())
    }
}
