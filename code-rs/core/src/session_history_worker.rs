//! Resource isolation for native history retrieval; the CLI re-enters this
//! module before starting its normal runtime, dotenv loading, or MCP servers.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use code_session_history::{Action, Request, Settings};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::protocol::SandboxPolicy;

pub const WORKER_ARG: &str = "--code-session-history-worker";
pub(crate) const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);
// Separate tabs share a resource budget, never a result channel or request.
static WORKER_SLOT: Semaphore = Semaphore::const_new(1);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Work {
    code_home: PathBuf,
    cwd: PathBuf,
    policy: SandboxPolicy,
    request: Request,
    staging_parent: Option<PathBuf>,
}

pub(crate) fn error(code: &str, message: impl ToString) -> Value {
    code_secrets::redact_json(&json!({
        "ok": false, "error": {"code": code, "message": message.to_string()}
    }))
}

pub(crate) async fn execute(
    code_home: &Path,
    cwd: &Path,
    policy: &SandboxPolicy,
    arguments: &str,
) -> Value {
    if arguments.len() > MAX_INPUT {
        return error("limit_exceeded", "session-history request exceeds 1 MiB");
    }
    let request: Value = match serde_json::from_str(arguments) {
        Ok(request) => request,
        Err(err) => return error("invalid_argument", err),
    };
    let staging = if request["action"] == "import_openai_export" {
        let data_dir = match Settings::data_dir(code_home).canonicalize() {
            Ok(path) => path,
            Err(err) => return error("source_error", err),
        };
        if !import_allowed(policy, cwd, &data_dir) {
            return error(
                "permission_denied",
                "Export import requires write permission for the session-history data directory; no files were changed.",
            );
        }
        let datasets = data_dir.join("datasets");
        if datasets
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return error("source_error", "dataset directory must not be a symlink");
        }
        if let Err(err) = std::fs::create_dir_all(&datasets) {
            return error("source_error", err);
        }
        match tempfile::Builder::new()
            .prefix(".history-worker-")
            .tempdir_in(&datasets)
        {
            Ok(staging) => Some(staging),
            Err(err) => return error("source_error", err),
        }
    } else {
        None
    };
    let input = match serde_json::to_vec(&json!({
        "code_home":code_home,"cwd":cwd,"policy":policy,"request":request,
        "staging_parent":staging.as_ref().map(|dir| dir.path())
    })) {
        Ok(input) if input.len() <= MAX_INPUT => input,
        _ => {
            return error(
                "limit_exceeded",
                "session-history worker input exceeds 1 MiB",
            );
        }
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(err) => return error("worker_unavailable", err),
    };
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    // Keep the supervisor alive to kill and reap its child even if the tool
    // future is cancelled. Dropping this request signals that supervisor.
    let supervisor = tokio::spawn(async move {
        let _staging = staging;
        let permit = tokio::select! {
            _ = cancellation.cancelled() => return error("cancelled", "history request cancelled"),
            permit = tokio::time::timeout(DEADLINE, WORKER_SLOT.acquire()) => match permit {
                Ok(Ok(permit)) => permit,
                _ => return error("resource_busy", "another history request is still using the worker slot"),
            }
        };
        let mut command = Command::new(executable);
        command.arg(WORKER_ARG);
        let result = supervise(command, input, cancellation, DEADLINE).await;
        drop(permit);
        result
    });
    match supervisor.await {
        Ok(value) => value,
        Err(err) => error("worker_failed", err),
    }
}

async fn supervise(
    mut command: Command,
    input: Vec<u8>,
    cancellation: CancellationToken,
    deadline: Duration,
) -> Value {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        #[cfg(target_os = "linux")]
        let parent = libc::getpid();
        #[cfg(target_os = "linux")]
        let parent_death_signal = linux_kernel();
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                if parent_death_signal && libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if parent_death_signal && libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                }
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return error("worker_unavailable", err),
    };
    let pid = child.id().expect("new child has a PID");
    #[cfg(target_os = "linux")]
    let watchdog = if linux_kernel() {
        crate::cgroup::spawn_exec_memory_watchdog_if_needed(pid, MEMORY_LIMIT)
    } else {
        None
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let exchange = async {
        stdin.write_all(&input).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let mut output = Vec::new();
        stdout
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut output)
            .await?;
        Ok::<_, std::io::Error>(output)
    };
    let mut result = tokio::select! {
        _ = cancellation.cancelled() => error("cancelled", "history request cancelled"),
        result = tokio::time::timeout(deadline, exchange) => match result {
            Err(_) => error("limit_exceeded", "session-history worker exceeded its wall-clock deadline"),
            Ok(Err(err)) => error("worker_failed", err),
            Ok(Ok(output)) if output.len() > MAX_OUTPUT => error("limit_exceeded", "session-history response exceeds 4 MiB"),
            Ok(Ok(output)) => match serde_json::from_slice::<Value>(&output) {
                Ok(value) if value.is_object() && value["ok"].is_boolean() => value,
                _ => error("worker_failed", "history worker exited without a valid response; a resource limit may have stopped it"),
            }
        }
    };
    let mut exited = false;
    if result["ok"].is_boolean()
        && result["error"]["code"] != "worker_failed"
        && result["error"]["code"] != "limit_exceeded"
        && result["error"]["code"] != "cancelled"
    {
        let status = tokio::select! {
            _ = cancellation.cancelled() => None,
            status = tokio::time::timeout(Duration::from_secs(1), child.wait()) => status.ok().and_then(Result::ok),
        };
        if let Some(status) = status {
            exited = true;
            if !status.success() {
                result = error(
                    "worker_failed",
                    format!("history worker exited with {status}"),
                );
            }
        } else {
            result = error(
                "worker_failed",
                "history worker did not exit after closing its response stream",
            );
        }
    }
    // Includes descendants in the worker's process group. Never signal Code's
    // own process group; all workers establish a fresh group before exec.
    if !exited {
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    #[cfg(target_os = "linux")]
    {
        let exceeded = watchdog
            .as_ref()
            .is_some_and(|watchdog| watchdog.limit_exceeded())
            || crate::cgroup::exec_cgroup_oom_killed(pid) == Some(true)
            || crate::cgroup::exec_cgroup_pids_limit_hit(pid) == Some(true);
        drop(watchdog);
        crate::cgroup::best_effort_cleanup_exec_cgroup(pid);
        if exceeded {
            return error(
                "limit_exceeded",
                "session-history worker exceeded its memory or process limit",
            );
        }
    }
    result
}

fn run(work: Work) -> Value {
    let settings = Settings::discover(&work.code_home);
    if work.request.action == Action::ImportOpenaiExport {
        let target = match settings.data_dir.canonicalize() {
            Ok(target) => target,
            Err(err) => {
                return error(
                    "source_error",
                    format!(
                        "Import directory unavailable; stage the export in the list_stores inbox first: {err}"
                    ),
                );
            }
        };
        if !import_allowed(&work.policy, &work.cwd, &target) {
            return error(
                "permission_denied",
                "Export import requires write permission for the session-history data directory; no files were changed.",
            );
        }
    }
    if let Some(staging) = work.staging_parent.as_ref() {
        let datasets = settings.data_dir.join("datasets").canonicalize();
        let valid =
            staging
                .canonicalize()
                .ok()
                .zip(datasets.ok())
                .is_some_and(|(staging, datasets)| {
                    staging.parent() == Some(datasets.as_path())
                        && staging.file_name().is_some_and(|name| {
                            name.to_string_lossy().starts_with(".history-worker-")
                        })
                });
        if !valid {
            return error(
                "invalid_argument",
                "invalid history import staging directory",
            );
        }
    }
    match code_session_history::execute_with_staging(
        settings,
        work.request,
        work.staging_parent.as_deref(),
    ) {
        Ok(mut value) => {
            value["ok"] = json!(true);
            value
        }
        Err(err) => error(err.code, err.message),
    }
}

pub(crate) fn import_allowed(policy: &SandboxPolicy, cwd: &Path, target: &Path) -> bool {
    policy.has_full_disk_write_access()
        || policy
            .get_writable_roots_with_cwd(cwd)
            .iter()
            .any(|root| root.is_path_writable(target))
}

#[cfg(target_os = "linux")]
fn linux_kernel() -> bool {
    let mut info = std::mem::MaybeUninit::<libc::utsname>::uninit();
    unsafe {
        libc::uname(info.as_mut_ptr()) == 0
            && std::ffi::CStr::from_ptr(info.assume_init().sysname.as_ptr()).to_bytes() == b"Linux"
    }
}

struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_OUTPUT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("history response exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Entry point used only by the executable's early argument dispatcher.
pub fn run_main() -> i32 {
    let limits = enforce_limits();
    let result = match limits.as_ref() {
        Err(err) => error("resource_enforcement_unavailable", err),
        Ok(_) => {
            let mut bytes = Vec::new();
            match std::io::stdin()
                .take((MAX_INPUT + 1) as u64)
                .read_to_end(&mut bytes)
            {
                Ok(_) if bytes.len() > MAX_INPUT => {
                    error("limit_exceeded", "history worker input exceeds 1 MiB")
                }
                Err(err) => error("invalid_argument", err),
                Ok(_) => match serde_json::from_slice::<Work>(&bytes) {
                    Ok(work) => run(work),
                    Err(err) => error("invalid_argument", err),
                },
            }
        }
    };
    let mut output = BoundedOutput(Vec::new());
    if serde_json::to_writer(&mut output, &result).is_err() {
        output.0.clear();
        let _ = serde_json::to_writer(
            &mut output,
            &error("limit_exceeded", "history worker response exceeds 4 MiB"),
        );
    }
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&output.0)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return 1;
    }
    0
}

#[cfg(all(unix, not(target_os = "macos")))]
fn enforce_limits() -> std::io::Result<()> {
    for (resource, limit) in [
        (libc::RLIMIT_AS, MEMORY_LIMIT),
        (libc::RLIMIT_CPU, DEADLINE.as_secs()),
        (libc::RLIMIT_CORE, 0),
    ] {
        let mut inherited = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(resource, inherited.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let inherited = unsafe { inherited.assume_init() };
        let limit = libc::rlimit {
            rlim_cur: inherited.rlim_cur.min(limit as libc::rlim_t),
            rlim_max: inherited.rlim_max.min(limit as libc::rlim_t),
        };
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "linux")]
    if linux_kernel() {
        crate::cgroup::best_effort_attach_self_to_exec_cgroup(
            std::process::id(),
            crate::cgroup::ExecCgroupLimits {
                memory_max_bytes: Some(MEMORY_LIMIT),
                pids_max: Some(32),
            },
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn enforce_limits() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native macOS lacks the required address-space limit; use a verified Cosmopolitan build for isolated history retrieval",
    ))
}

#[cfg(windows)]
fn enforce_limits() -> std::io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = std::os::windows::io::OwnedHandle::from_raw_handle(handle);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_TIME;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.BasicLimitInformation.PerProcessUserTimeLimit =
            DEADLINE.as_secs() as i64 * 10_000_000;
        limits.ProcessMemoryLimit = MEMORY_LIMIT as usize;
        if SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
            || AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }
}

#[cfg(not(any(unix, windows)))]
fn enforce_limits() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this platform cannot enforce native history worker limits",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_worker_output_writer_rejects_growth_at_the_boundary() {
        let mut writer = BoundedOutput(vec![0; MAX_OUTPUT - 1]);
        assert_eq!(writer.write(b"x").unwrap(), 1);
        assert!(writer.write(b"x").is_err());
        assert_eq!(writer.0.len(), MAX_OUTPUT);
    }

    #[cfg(unix)]
    fn python(script: &str) -> Command {
        let mut command = Command::new("python3");
        command.args(["-c", script]);
        command
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn history_worker_supervisor_rejects_oversized_and_invalid_output() {
        let large = supervise(
            python("import sys; sys.stdin.read(); sys.stdout.write('x' * (5 * 1024 * 1024))"),
            vec![],
            CancellationToken::new(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(large["error"]["code"], "limit_exceeded");
        let invalid = supervise(
            python("import sys; sys.stdin.read(); print('not json')"),
            vec![],
            CancellationToken::new(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(invalid["error"]["code"], "worker_failed");
        let crashed = supervise(
            python("import sys; sys.stdin.read(); print('{\"ok\":true}'); sys.exit(7)"),
            vec![],
            CancellationToken::new(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(crashed["error"]["code"], "worker_failed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn history_worker_supervisor_enforces_timeout_and_cancellation_and_reaps_children() {
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let pid_file = directory.path().join("pid");
            let mut command = python(
                "import os,sys,time; sys.stdin.read(); open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(30)",
            );
            command.arg(&pid_file);
            let token = CancellationToken::new();
            let token_for_task = token.clone();
            let task = tokio::spawn(async move {
                supervise(command, vec![], token_for_task, Duration::from_secs(2)).await
            });
            let pid = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if let Some(pid) = std::fs::read_to_string(&pid_file).ok()
                        .and_then(|text| text.parse::<libc::pid_t>().ok())
                    {
                        break pid;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if cancel {
                token.cancel();
            }
            let result = task.await.unwrap();
            assert_eq!(
                result["error"]["code"],
                if cancel {
                    "cancelled"
                } else {
                    "limit_exceeded"
                }
            );
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "worker must be gone after its supervisor returns"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn history_worker_responses_remain_owned_with_concurrent_supervisors() {
        let script = "import sys,json; value=json.load(sys.stdin); print(json.dumps({'ok':True,'owner':value['owner']}))";
        let (first, second) = tokio::join!(
            supervise(
                python(script),
                br#"{"owner":"alpha"}"#.to_vec(),
                CancellationToken::new(),
                Duration::from_secs(5)
            ),
            supervise(
                python(script),
                br#"{"owner":"bravo"}"#.to_vec(),
                CancellationToken::new(),
                Duration::from_secs(5)
            ),
        );
        assert_eq!(first["owner"], "alpha");
        assert_eq!(second["owner"], "bravo");
    }
}
