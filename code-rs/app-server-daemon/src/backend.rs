use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;

use crate::DaemonPaths;
use crate::ProcessIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendState {
    NotRunning,
    Running(ProcessIdentity),
    Stale(ProcessIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub executable: PathBuf,
    pub code_home: PathBuf,
    pub socket_path: PathBuf,
    pub stderr_log_file: PathBuf,
    pub listen_url: String,
    pub remote_control_enabled: bool,
}

impl StartRequest {
    pub(crate) fn new(paths: &DaemonPaths, remote_control_enabled: bool) -> Self {
        Self {
            executable: paths.executable.clone(),
            code_home: paths.code_home.clone(),
            socket_path: paths.socket_path.clone(),
            stderr_log_file: paths.stderr_log_file.clone(),
            listen_url: format!("unix://{}", paths.socket_path.display()),
            remote_control_enabled,
        }
    }
}

#[async_trait]
pub trait ProcessBackend: Send + Sync {
    async fn state(&self) -> Result<BackendState>;
    async fn start(&self, request: StartRequest) -> Result<ProcessIdentity>;
    async fn stop(&self, identity: &ProcessIdentity) -> Result<()>;
}
