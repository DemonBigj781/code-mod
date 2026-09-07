use std::future::Future;
use std::io::Write;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use code_app_server::AppServerRuntimeOptions;
use code_app_server::AppServerTransport;
use code_app_server::RemoteControlStartupMode;
use code_app_server_daemon::LifecycleCommand;
use code_app_server_daemon::LifecycleOutput;
use code_app_server_daemon::LifecycleStatus;
use code_app_server_daemon::RemoteControlReadyOutput;
use code_app_server_daemon::RemoteControlReadyStatus;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use code_common::CliConfigOverrides;
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const FOREGROUND_SOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FOREGROUND_SOCKET_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const FOREGROUND_APP_SERVER_ABORT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Args)]
pub struct RemoteControlCommand {
    /// Emit machine-readable JSON.
    #[arg(long = "json", global = true)]
    json: bool,

    #[command(subcommand)]
    subcommand: Option<RemoteControlSubcommand>,
}

impl RemoteControlCommand {
    pub fn json(&self) -> bool {
        self.json
    }

    pub fn subcommand_name(&self) -> &'static str {
        match self.subcommand {
            None => "remote-control",
            Some(RemoteControlSubcommand::Start) => "remote-control start",
            Some(RemoteControlSubcommand::Stop) => "remote-control stop",
            Some(RemoteControlSubcommand::Pair) => "remote-control pair",
        }
    }
}

#[derive(Debug, Clone, Copy, clap::Subcommand)]
enum RemoteControlSubcommand {
    /// Start the app-server daemon with remote control enabled.
    Start,

    /// Stop the app-server daemon.
    Stop,

    /// Create and print a short-lived manual pairing code.
    Pair,
}

pub async fn run(
    command: RemoteControlCommand,
    code_linux_sandbox_exe: Option<std::path::PathBuf>,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    match command.subcommand {
        None => {
            print_remote_control_progress(
                command.json,
                "Starting app-server with remote control enabled...",
            )?;
            run_foreground_remote_control(
                command.json,
                code_linux_sandbox_exe,
                root_config_overrides,
            )
            .await?;
        }
        Some(RemoteControlSubcommand::Start) => {
            print_remote_control_progress(
                command.json,
                "Starting app-server daemon with remote control enabled...",
            )?;
            let output = code_app_server_daemon::ensure_remote_control_ready()
                .await
                .context("failed to start remote control")?;
            println!(
                "{}",
                format_remote_control_start_output(&output, command.json)?
            );
        }
        Some(RemoteControlSubcommand::Stop) => {
            print_remote_control_progress(command.json, "Stopping remote control...")?;
            let output = code_app_server_daemon::run(LifecycleCommand::Stop)
                .await
                .context("failed to stop remote control")?;
            println!(
                "{}",
                format_remote_control_stop_output(&output, command.json)?
            );
        }
        Some(RemoteControlSubcommand::Pair) => {
            let output = start_remote_control_pairing_after_ready(
                code_app_server_daemon::ensure_remote_control_ready,
                code_app_server_daemon::start_remote_control_pairing,
            )
            .await?;
            println!(
                "{}",
                format_remote_control_pairing_output(&output, command.json)?
            );
        }
    }
    Ok(())
}

async fn start_remote_control_pairing_after_ready<
    EnsureReady,
    EnsureReadyFuture,
    StartPairing,
    StartPairingFuture,
>(
    ensure_ready: EnsureReady,
    start_pairing: StartPairing,
) -> anyhow::Result<RemoteControlPairingStartResponse>
where
    EnsureReady: FnOnce() -> EnsureReadyFuture,
    EnsureReadyFuture: Future<Output = anyhow::Result<RemoteControlReadyOutput>>,
    StartPairing: FnOnce() -> StartPairingFuture,
    StartPairingFuture: Future<Output = anyhow::Result<RemoteControlPairingStartResponse>>,
{
    let ready = ensure_ready()
        .await
        .context("failed to prepare remote control for pairing")?;
    ensure_remote_control_startable(&ready.remote_control)?;
    start_pairing()
        .await
        .context("failed to start remote-control pairing")
}

fn print_remote_control_progress(json: bool, message: &str) -> anyhow::Result<()> {
    if json {
        return Ok(());
    }

    println!("{message}");
    std::io::stdout()
        .flush()
        .context("failed to flush remote-control progress message")?;
    Ok(())
}

async fn run_foreground_remote_control(
    json: bool,
    code_linux_sandbox_exe: Option<std::path::PathBuf>,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    let socket_dir = tempfile::Builder::new()
        .prefix("code-rc-")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::tempdir())
        .context("failed to create private app-server socket directory")?;
    let socket_path = socket_dir.path().join("rc.sock");
    let transport = AppServerTransport::UnixSocket {
        socket_path: socket_path.clone(),
    };
    let runtime_options = AppServerRuntimeOptions {
        remote_control_startup_mode: RemoteControlStartupMode::EnabledEphemeral,
        install_shutdown_signal_handler: false,
    };
    let (stop_rx, stop_signal_task) = foreground_stop_signal();
    let mut app_server_task = tokio::spawn(code_app_server::run_main_with_transport_options(
        code_linux_sandbox_exe,
        root_config_overrides,
        transport,
        runtime_options,
    ));

    let summary = match wait_for_foreground_remote_control_start(
        &mut app_server_task,
        code_app_server_daemon::enable_remote_control_on_socket(
            &socket_path,
            FOREGROUND_SOCKET_CONNECT_TIMEOUT,
            FOREGROUND_SOCKET_CONNECT_RETRY_DELAY,
        ),
        stop_rx.clone(),
    )
    .await
    {
        ForegroundStartupResult::Ready(summary) => summary,
        ForegroundStartupResult::Stopped => {
            let result = abort_foreground_app_server(app_server_task).await;
            cancel_signal_task(stop_signal_task).await;
            return result;
        }
        ForegroundStartupResult::ReadyFailed(error) => {
            let result = attach_abort_error(error, app_server_task).await;
            cancel_signal_task(stop_signal_task).await;
            return Err(result);
        }
        ForegroundStartupResult::AppServerExited(error) => {
            cancel_signal_task(stop_signal_task).await;
            return Err(error);
        }
    };

    if *stop_rx.borrow() {
        let result = abort_foreground_app_server(app_server_task).await;
        cancel_signal_task(stop_signal_task).await;
        return result;
    }

    let output = match format_foreground_ready_output(&summary, json) {
        Ok(output) => output,
        Err(error) => {
            let result = attach_abort_error(error, app_server_task).await;
            cancel_signal_task(stop_signal_task).await;
            return Err(result);
        }
    };
    println!("{output}");

    let result = wait_for_foreground_app_server(app_server_task, stop_rx).await;
    cancel_signal_task(stop_signal_task).await;
    result
}

fn foreground_stop_signal() -> (watch::Receiver<bool>, JoinHandle<()>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        if let Err(error) = wait_for_foreground_stop_signal().await {
            eprintln!("failed to listen for foreground shutdown signal: {error}");
        }
        let _ = stop_tx.send(true);
    });
    (stop_rx, task)
}

#[cfg(unix)]
async fn wait_for_foreground_stop_signal() -> std::io::Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        signal = sigterm.recv() => match signal {
            Some(()) => Ok(()),
            None => Err(std::io::Error::other("SIGTERM listener closed unexpectedly")),
        },
    }
}

#[cfg(not(unix))]
async fn wait_for_foreground_stop_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

async fn cancel_signal_task(task: JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

enum ForegroundStartupResult {
    Ready(RemoteControlReadyStatus),
    Stopped,
    ReadyFailed(anyhow::Error),
    AppServerExited(anyhow::Error),
}

async fn wait_for_foreground_remote_control_start<F>(
    app_server_task: &mut JoinHandle<std::io::Result<()>>,
    ready: F,
    mut stop_rx: watch::Receiver<bool>,
) -> ForegroundStartupResult
where
    F: Future<Output = anyhow::Result<RemoteControlReadyStatus>>,
{
    tokio::pin!(ready);

    tokio::select! {
        ready_result = &mut ready => match ready_result {
            Ok(summary) => ForegroundStartupResult::Ready(summary),
            Err(error) => ForegroundStartupResult::ReadyFailed(error),
        },
        app_server_result = app_server_task => {
            ForegroundStartupResult::AppServerExited(
                foreground_app_server_exited_before_ready(app_server_result)
            )
        }
        _ = wait_for_stop_signal(&mut stop_rx) => ForegroundStartupResult::Stopped,
    }
}

async fn wait_for_foreground_app_server(
    mut app_server_task: JoinHandle<std::io::Result<()>>,
    mut stop_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    tokio::select! {
        app_server_result = &mut app_server_task => {
            app_server_result
                .context("foreground app-server task failed to join")?
                .context("foreground app-server exited with an error")?;
        }
        _ = wait_for_stop_signal(&mut stop_rx) => {
            abort_foreground_app_server(app_server_task).await?;
        }
    }

    Ok(())
}

async fn wait_for_stop_signal(stop_rx: &mut watch::Receiver<bool>) {
    if *stop_rx.borrow() {
        return;
    }
    let _ = stop_rx.wait_for(|stopped| *stopped).await;
}

fn foreground_app_server_exited_before_ready(
    result: Result<std::io::Result<()>, tokio::task::JoinError>,
) -> anyhow::Error {
    match result {
        Ok(Ok(())) => {
            anyhow::anyhow!("foreground app-server exited before remote control became ready")
        }
        Ok(Err(error)) => anyhow::Error::new(error)
            .context("foreground app-server exited before remote control became ready"),
        Err(error) => anyhow::Error::new(error)
            .context("foreground app-server task failed before remote control became ready"),
    }
}

async fn abort_foreground_app_server(
    app_server_task: JoinHandle<std::io::Result<()>>,
) -> anyhow::Result<()> {
    app_server_task.abort();
    match timeout(FOREGROUND_APP_SERVER_ABORT_TIMEOUT, app_server_task).await {
        Ok(Err(error)) if error.is_cancelled() => Ok(()),
        Ok(Err(error)) => Err(anyhow::Error::new(error)
            .context("foreground app-server task failed while stopping")),
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => {
            Err(anyhow::Error::new(error).context("foreground app-server failed while stopping"))
        }
        Err(_) => anyhow::bail!("timed out stopping foreground app-server"),
    }
}

async fn attach_abort_error(
    original: anyhow::Error,
    app_server_task: JoinHandle<std::io::Result<()>>,
) -> anyhow::Error {
    match abort_foreground_app_server(app_server_task).await {
        Ok(()) => original,
        Err(cleanup) => original.context(format!(
            "additionally failed to stop foreground app-server: {cleanup:#}"
        )),
    }
}

fn format_remote_control_start_output(
    output: &RemoteControlReadyOutput,
    json: bool,
) -> anyhow::Result<String> {
    ensure_remote_control_startable(&output.remote_control)?;
    if json {
        return Ok(serde_json::to_string(&RemoteControlStartJsonOutput::daemon(
            output,
        ))?);
    }

    let mut lines = remote_control_start_human_lines(
        &output.remote_control,
        RemoteControlHumanOutputMode::Daemon,
    )?;
    lines.extend(daemon_app_server_human_lines(&output.daemon));
    Ok(lines.join("\n"))
}

fn format_foreground_ready_output(
    summary: &RemoteControlReadyStatus,
    json: bool,
) -> anyhow::Result<String> {
    ensure_remote_control_startable(summary)?;
    if json {
        return Ok(serde_json::to_string(
            &RemoteControlStartJsonOutput::foreground(summary),
        )?);
    }

    Ok(remote_control_start_human_lines(
        summary,
        RemoteControlHumanOutputMode::Foreground,
    )?
    .join("\n"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlStartJsonOutput<'a> {
    mode: RemoteControlModeJson,
    status: RemoteControlConnectionStatus,
    server_name: &'a str,
    environment_id: Option<&'a str>,
    timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    daemon: Option<&'a LifecycleOutput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RemoteControlModeJson {
    Foreground,
    Daemon,
}

impl<'a> RemoteControlStartJsonOutput<'a> {
    fn foreground(summary: &'a RemoteControlReadyStatus) -> Self {
        Self {
            mode: RemoteControlModeJson::Foreground,
            status: summary.status,
            server_name: &summary.server_name,
            environment_id: summary.environment_id.as_deref(),
            timed_out: summary.status == RemoteControlConnectionStatus::Connecting,
            daemon: None,
        }
    }

    fn daemon(output: &'a RemoteControlReadyOutput) -> Self {
        let remote_control = &output.remote_control;
        Self {
            mode: RemoteControlModeJson::Daemon,
            status: remote_control.status,
            server_name: &remote_control.server_name,
            environment_id: remote_control.environment_id.as_deref(),
            timed_out: remote_control.status == RemoteControlConnectionStatus::Connecting,
            daemon: Some(&output.daemon),
        }
    }
}

fn ensure_remote_control_startable(output: &RemoteControlReadyStatus) -> anyhow::Result<()> {
    match output.status {
        RemoteControlConnectionStatus::Connected | RemoteControlConnectionStatus::Connecting => {
            Ok(())
        }
        RemoteControlConnectionStatus::Errored => anyhow::bail!(
            "Remote control could not connect on {}. Verify that you are signed in with ChatGPT using `code login`; API key authentication is not supported.",
            output.server_name
        ),
        RemoteControlConnectionStatus::Disabled => {
            anyhow::bail!("Remote control is disabled on {}.", output.server_name)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlHumanOutputMode {
    Foreground,
    Daemon,
}

fn remote_control_start_human_lines(
    summary: &RemoteControlReadyStatus,
    mode: RemoteControlHumanOutputMode,
) -> anyhow::Result<Vec<String>> {
    ensure_remote_control_startable(summary)?;
    let mut lines = vec![match summary.status {
        RemoteControlConnectionStatus::Connected => format!(
            "This machine is available for remote control as {}.",
            summary.server_name
        ),
        RemoteControlConnectionStatus::Connecting => format!(
            "Remote control is enabled on {} and still connecting.",
            summary.server_name
        ),
        RemoteControlConnectionStatus::Errored | RemoteControlConnectionStatus::Disabled => {
            unreachable!("terminal failure statuses are rejected before formatting")
        }
    }];
    if mode == RemoteControlHumanOutputMode::Foreground {
        lines.push("Press Ctrl-C to stop.".to_string());
    }
    Ok(lines)
}

fn daemon_app_server_human_lines(output: &LifecycleOutput) -> Vec<String> {
    let mut lines = vec![format!(
        "Daemon status: {}.",
        lifecycle_status_name(output.status)
    )];
    if let Some(pid) = output.pid {
        lines.push(format!("App-server PID: {pid}."));
    }
    if let Some(version) = output.app_server_version.as_deref() {
        lines.push(format!("App-server version: {version}."));
    }
    lines
}

fn lifecycle_status_name(status: LifecycleStatus) -> &'static str {
    match status {
        LifecycleStatus::AlreadyRunning => "already running",
        LifecycleStatus::Started => "started",
        LifecycleStatus::Restarted => "restarted",
        LifecycleStatus::Stopped => "stopped",
        LifecycleStatus::NotRunning => "not running",
        LifecycleStatus::Running => "running",
    }
}

fn format_remote_control_stop_output(
    output: &LifecycleOutput,
    json: bool,
) -> anyhow::Result<String> {
    if json {
        return Ok(serde_json::to_string(output)?);
    }

    Ok(match output.status {
        LifecycleStatus::Stopped => "Remote control stopped.".to_string(),
        LifecycleStatus::NotRunning => "Remote control is not running.".to_string(),
        status => format!(
            "Remote control stop completed with status {}.",
            lifecycle_status_name(status)
        ),
    })
}

fn format_remote_control_pairing_output(
    output: &RemoteControlPairingStartResponse,
    json: bool,
) -> anyhow::Result<String> {
    if json {
        return Ok(serde_json::to_string(output)?);
    }

    let manual_pairing_code = output
        .manual_pairing_code
        .as_deref()
        .context("remote-control pairing response did not include a manual pairing code")?;
    Ok(format!("Pairing code: {manual_pairing_code}"))
}

#[cfg(test)]
#[path = "remote_control_cmd_tests.rs"]
mod tests;
