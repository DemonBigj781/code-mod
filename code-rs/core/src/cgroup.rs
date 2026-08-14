#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use std::sync::{OnceLock, RwLock};

#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";

#[cfg(target_os = "linux")]
const EXEC_CGROUP_SUBDIR: &str = "code-exec";

#[cfg(target_os = "linux")]
const EXEC_CGROUP_OOM_SCORE_ADJ: &str = "500";

#[cfg(target_os = "linux")]
const EXEC_MEMORY_WATCHDOG_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(250);

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct ExecMemoryWatchdog {
    memory_max_bytes: u64,
    limit_exceeded: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(target_os = "linux")]
impl ExecMemoryWatchdog {
    pub(crate) fn memory_max_bytes(&self) -> u64 {
        self.memory_max_bytes
    }

    pub(crate) fn limit_exceeded(&self) -> bool {
        self.limit_exceeded.load(Ordering::Acquire)
    }
}

#[cfg(target_os = "linux")]
impl Drop for ExecMemoryWatchdog {
    fn drop(&mut self) {
        // A detached watchdog could later target a reused process-group ID.
        self.task.abort();
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ExecCgroupLimits {
    pub(crate) memory_max_bytes: Option<u64>,
    pub(crate) pids_max: Option<u64>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ExecLimitOverride {
    #[default]
    Auto,
    Disabled,
    Value(u64),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ExecCgroupLimitOverrides {
    pub(crate) memory_max_bytes: ExecLimitOverride,
    pub(crate) pids_max: ExecLimitOverride,
}

#[cfg(target_os = "linux")]
static EXEC_CGROUP_LIMIT_OVERRIDES: OnceLock<RwLock<ExecCgroupLimitOverrides>> = OnceLock::new();

#[cfg(target_os = "linux")]
pub(crate) fn set_exec_cgroup_limit_overrides(overrides: ExecCgroupLimitOverrides) {
    let lock = EXEC_CGROUP_LIMIT_OVERRIDES
        .get_or_init(|| RwLock::new(ExecCgroupLimitOverrides::default()));
    if let Ok(mut guard) = lock.write() {
        *guard = overrides;
    }
}

#[cfg(target_os = "linux")]
fn exec_cgroup_limit_overrides_snapshot() -> ExecCgroupLimitOverrides {
    let lock = EXEC_CGROUP_LIMIT_OVERRIDES
        .get_or_init(|| RwLock::new(ExecCgroupLimitOverrides::default()));
    lock.read().map(|guard| *guard).unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub(crate) fn default_exec_memory_max_bytes() -> Option<u64> {
    match exec_cgroup_limit_overrides_snapshot().memory_max_bytes {
        ExecLimitOverride::Disabled => return None,
        ExecLimitOverride::Value(value) => return Some(value),
        ExecLimitOverride::Auto => {}
    }

    auto_exec_memory_max_bytes()
}

#[cfg(target_os = "linux")]
pub(crate) fn auto_exec_memory_max_bytes() -> Option<u64> {
    if let Ok(raw) = std::env::var("CODEX_EXEC_MEMORY_MAX_BYTES") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            if value > 0 {
                return Some(value);
            }
        }
    }
    if let Ok(raw) = std::env::var("CODEX_EXEC_MEMORY_MAX_MB") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            if value > 0 {
                return Some(value.saturating_mul(1024 * 1024));
            }
        }
    }

    let available = read_mem_available_bytes()?;
    // Leave headroom for the parent TUI + other background processes.
    // Keep the cap within a reasonable range so we still protect the parent
    // on larger machines.
    let sixty_percent = available.saturating_mul(60) / 100;
    let min = 512_u64.saturating_mul(1024 * 1024);
    let max = 4_u64.saturating_mul(1024 * 1024 * 1024);
    Some(sixty_percent.clamp(min, max))
}

#[cfg(target_os = "linux")]
fn default_exec_pids_max_for_cpus(cpus: u64) -> u64 {
    // Start small by default (protect the parent) but scale a bit with cores.
    // Clamp to keep it reasonable across small and large machines.
    cpus.saturating_mul(64).clamp(256, 4096)
}

#[cfg(target_os = "linux")]
pub(crate) fn default_exec_pids_max() -> Option<u64> {
    match exec_cgroup_limit_overrides_snapshot().pids_max {
        ExecLimitOverride::Disabled => return None,
        ExecLimitOverride::Value(value) => return Some(value),
        ExecLimitOverride::Auto => {}
    }

    auto_exec_pids_max()
}

#[cfg(target_os = "linux")]
pub(crate) fn auto_exec_pids_max() -> Option<u64> {
    if let Ok(raw) = std::env::var("CODEX_EXEC_PIDS_MAX") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            if value >= 1 {
                return Some(value);
            }
        }
    }

    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(4);
    Some(default_exec_pids_max_for_cpus(cpus))
}

#[cfg(target_os = "linux")]
pub(crate) fn exec_pids_max_with_override(override_: ExecLimitOverride) -> Option<u64> {
    match override_ {
        ExecLimitOverride::Disabled => None,
        ExecLimitOverride::Value(value) => Some(value),
        ExecLimitOverride::Auto => auto_exec_pids_max(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn exec_memory_max_bytes_with_override(override_: ExecLimitOverride) -> Option<u64> {
    match override_ {
        ExecLimitOverride::Disabled => None,
        ExecLimitOverride::Value(value) => Some(value),
        ExecLimitOverride::Auto => auto_exec_memory_max_bytes(),
    }
}

#[cfg(target_os = "linux")]
fn read_mem_available_bytes() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn is_cgroup_v2() -> bool {
    std::fs::metadata(Path::new(CGROUP_MOUNT).join("cgroup.controllers")).is_ok()
}

#[cfg(target_os = "linux")]
fn current_cgroup_relative() -> Option<PathBuf> {
    let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in contents.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return None;
            }
            return Some(PathBuf::from(trimmed.trim_start_matches('/')));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn exec_cgroup_parent_abs() -> Option<PathBuf> {
    if !is_cgroup_v2() {
        return None;
    }
    let rel = current_cgroup_relative()?;
    Some(Path::new(CGROUP_MOUNT).join(rel).join(EXEC_CGROUP_SUBDIR))
}

#[cfg(target_os = "linux")]
pub(crate) fn exec_cgroup_abs_for_pid(pid: u32) -> Option<PathBuf> {
    exec_cgroup_parent_abs().map(|parent| parent.join(format!("pid-{pid}")))
}

#[cfg(target_os = "linux")]
fn best_effort_enable_memory_controller(parent: &Path) {
    let controllers = std::fs::read_to_string(parent.join("cgroup.controllers")).ok();
    if controllers.as_deref().unwrap_or_default().split_whitespace().all(|c| c != "memory") {
        return;
    }
    let subtree = parent.join("cgroup.subtree_control");
    let _ = std::fs::write(subtree, "+memory");
}

#[cfg(target_os = "linux")]
fn best_effort_enable_pids_controller(parent: &Path) {
    let controllers = std::fs::read_to_string(parent.join("cgroup.controllers")).ok();
    if controllers.as_deref().unwrap_or_default().split_whitespace().all(|c| c != "pids") {
        return;
    }
    let subtree = parent.join("cgroup.subtree_control");
    let _ = std::fs::write(subtree, "+pids");
}

#[cfg(target_os = "linux")]
fn best_effort_attach_pid_to_exec_cgroup_inner(
    pid: u32,
    limits: ExecCgroupLimits,
    set_self_oom_score_adj: bool,
) {
    let Some(parent) = exec_cgroup_parent_abs() else {
        return;
    };

    let _ = std::fs::create_dir_all(&parent);
    if limits.memory_max_bytes.is_some() {
        best_effort_enable_memory_controller(&parent);
    }
    if limits.pids_max.is_some() {
        best_effort_enable_pids_controller(&parent);
    }

    let cgroup_dir = parent.join(format!("pid-{pid}"));
    if std::fs::create_dir_all(&cgroup_dir).is_err() {
        return;
    }

    let mut attached = false;

    if let Some(memory_max_bytes) = limits.memory_max_bytes {
        let memory_max_path = cgroup_dir.join("memory.max");
        if memory_max_path.exists() {
            let _ = std::fs::write(&memory_max_path, memory_max_bytes.to_string());
            attached = true;

            let oom_group_path = cgroup_dir.join("memory.oom.group");
            if oom_group_path.exists() {
                let _ = std::fs::write(oom_group_path, "1");
            }

            if set_self_oom_score_adj {
                // Prefer killing the exec subtree first if the host does hit global OOM.
                let _ = std::fs::write("/proc/self/oom_score_adj", EXEC_CGROUP_OOM_SCORE_ADJ);
            }
        }
    }

    if let Some(pids_max) = limits.pids_max {
        let pids_max_path = cgroup_dir.join("pids.max");
        if pids_max_path.exists() {
            let _ = std::fs::write(&pids_max_path, pids_max.to_string());
            attached = true;
        }
    }

    if !attached {
        return;
    }

    let procs_path = cgroup_dir.join("cgroup.procs");
    if procs_path.exists() {
        let _ = std::fs::write(procs_path, pid.to_string());
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn best_effort_attach_self_to_exec_cgroup(pid: u32, limits: ExecCgroupLimits) {
    best_effort_attach_pid_to_exec_cgroup_inner(pid, limits, true);
}

#[cfg(target_os = "linux")]
pub(crate) fn best_effort_attach_pid_to_exec_cgroup(pid: u32, limits: ExecCgroupLimits) {
    best_effort_attach_pid_to_exec_cgroup_inner(pid, limits, false);
}

#[cfg(target_os = "linux")]
pub(crate) fn exec_cgroup_oom_killed(pid: u32) -> Option<bool> {
    let dir = exec_cgroup_abs_for_pid(pid)?;
    let contents = std::fs::read_to_string(dir.join("memory.events")).ok()?;
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(val) = parts.next() else {
            continue;
        };
        if key == "oom_kill" {
            if let Ok(parsed) = val.parse::<u64>() {
                return Some(parsed > 0);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub(crate) fn exec_cgroup_memory_max_bytes(pid: u32) -> Option<u64> {
    let dir = exec_cgroup_abs_for_pid(pid)?;
    let raw = std::fs::read_to_string(dir.join("memory.max")).ok()?;
    let trimmed = raw.trim();
    if trimmed == "max" {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
fn exec_cgroup_contains_pid(pid: u32) -> bool {
    let Some(dir) = exec_cgroup_abs_for_pid(pid) else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(dir.join("cgroup.procs")) else {
        return false;
    };
    contents
        .lines()
        .any(|line| line.trim().parse::<u32>().ok() == Some(pid))
}

#[cfg(target_os = "linux")]
fn exec_cgroup_memory_limit_is_active(pid: u32) -> bool {
    exec_cgroup_memory_max_bytes(pid).is_some() && exec_cgroup_contains_pid(pid)
}

#[cfg(target_os = "linux")]
fn process_group_id_from_stat(contents: &str) -> Option<u32> {
    // `/proc/<pid>/stat` starts with `pid (comm) state ppid pgrp ...`.
    // `comm` may contain spaces and parentheses, so split only after its final `)`.
    contents
        .get(contents.rfind(')')? + 1..)?
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn process_resident_bytes(proc_dir: &Path, page_size: u64) -> Option<u64> {
    let statm = std::fs::read_to_string(proc_dir.join("statm")).ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(resident_pages.saturating_mul(page_size))
}

#[cfg(target_os = "linux")]
fn process_group_resident_bytes(process_group_id: u32) -> u64 {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size).unwrap_or(4096);
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        })
        .filter_map(|entry| {
            let proc_dir = entry.path();
            let stat = std::fs::read_to_string(proc_dir.join("stat")).ok()?;
            (process_group_id_from_stat(&stat) == Some(process_group_id))
                .then(|| process_resident_bytes(&proc_dir, page_size))?
        })
        .fold(0_u64, u64::saturating_add)
}

#[cfg(target_os = "linux")]
fn spawn_process_group_memory_watchdog(
    process_group_id: u32,
    memory_max_bytes: u64,
) -> ExecMemoryWatchdog {
    let limit_exceeded = Arc::new(AtomicBool::new(false));
    let limit_exceeded_for_task = Arc::clone(&limit_exceeded);
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(EXEC_MEMORY_WATCHDOG_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let resident_bytes = process_group_resident_bytes(process_group_id);
            if resident_bytes <= memory_max_bytes {
                continue;
            }

            limit_exceeded_for_task.store(true, Ordering::Release);
            tracing::warn!(
                process_group_id,
                resident_bytes,
                memory_max_bytes,
                "exec process group exceeded fallback memory limit; killing it"
            );

            let pgid = process_group_id as libc::pid_t;
            if pgid != unsafe { libc::getpgrp() } {
                let _ = unsafe { libc::killpg(pgid, libc::SIGKILL) };
            }
            break;
        }
    });

    ExecMemoryWatchdog {
        memory_max_bytes,
        limit_exceeded,
        task,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn spawn_exec_memory_watchdog_if_needed(
    process_group_id: u32,
    memory_max_bytes: u64,
) -> Option<ExecMemoryWatchdog> {
    if exec_cgroup_memory_limit_is_active(process_group_id) {
        return None;
    }

    tracing::warn!(
        process_group_id,
        memory_max_bytes,
        "exec cgroup memory limit is unavailable; enabling process-group memory watchdog"
    );
    Some(spawn_process_group_memory_watchdog(
        process_group_id,
        memory_max_bytes,
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn best_effort_cleanup_exec_cgroup(pid: u32) {
    let Some(dir) = exec_cgroup_abs_for_pid(pid) else {
        return;
    };
    // Only remove the per-pid directory. The parent container stays.
    let _ = std::fs::remove_dir(&dir);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;

    #[test]
    fn default_exec_pids_max_for_cpus_clamps_low_and_high() {
        assert_eq!(default_exec_pids_max_for_cpus(1), 256);
        assert_eq!(default_exec_pids_max_for_cpus(8), 512);
        assert_eq!(default_exec_pids_max_for_cpus(64), 4096);
    }

    #[test]
    fn parses_process_group_after_a_complex_proc_stat_name() {
        let stat = "123 (worker name with ) paren) S 45 678 9 10";
        assert_eq!(process_group_id_from_stat(stat), Some(678));
    }

    #[tokio::test]
    async fn fallback_memory_watchdog_kills_the_process_group_before_host_oom() {
        let mut command = tokio::process::Command::new("python3");
        command.args([
            "-c",
            "import time; payload = bytearray(64 * 1024 * 1024); time.sleep(30)",
        ]);
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command.spawn().expect("spawn memory test child");
        let pid = child.id().expect("memory test child pid");
        let watchdog = spawn_exec_memory_watchdog_if_needed(pid, 32 * 1024 * 1024)
            .expect("unattached child should use fallback memory watchdog");

        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("watchdog should stop child before timeout")
            .expect("wait for memory test child");

        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(watchdog.limit_exceeded());
    }
}
