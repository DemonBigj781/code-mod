use std::sync::Mutex as StdMutex;

use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::network_approval::NetworkAttemptGuard;

pub(crate) struct ExecCommandSessionParts {
    pub(crate) killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    #[cfg(unix)]
    pub(crate) process_group_id: Option<u32>,
    #[cfg(target_os = "linux")]
    pub(crate) cgroup_pid: Option<u32>,
    #[cfg(target_os = "linux")]
    pub(crate) memory_watchdog: Option<crate::cgroup::ExecMemoryWatchdog>,
    pub(crate) reader_handle: JoinHandle<()>,
    pub(crate) writer_handle: JoinHandle<()>,
    pub(crate) wait_handle: JoinHandle<()>,
    pub(crate) exit_code: std::sync::Arc<StdMutex<Option<i32>>>,
    pub(crate) network_attempt_guard: Option<NetworkAttemptGuard>,
}

#[derive(Debug)]
pub(crate) struct ExecCommandSession {
    /// Queue for writing bytes to the process stdin (PTY master write side).
    writer_tx: mpsc::Sender<Vec<u8>>,
    /// Broadcast stream of output chunks read from the PTY. New subscribers
    /// receive only chunks emitted after they subscribe.
    output_tx: broadcast::Sender<Vec<u8>>,

    #[cfg(unix)]
    /// Cached process group id so drop can hard-kill descendants on Unix.
    process_group_id: Option<u32>,

    #[cfg(target_os = "linux")]
    /// PID used for Linux exec cgroup cleanup (best-effort).
    cgroup_pid: Option<u32>,

    #[cfg(target_os = "linux")]
    /// Fallback RSS monitor used when the configured cgroup limit is unavailable.
    memory_watchdog: Option<crate::cgroup::ExecMemoryWatchdog>,

    /// Child killer handle for termination on drop (can signal independently
    /// of a thread blocked in `.wait()`).
    killer: StdMutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,

    /// `JoinHandle` for the blocking PTY reader task.
    reader_handle: StdMutex<Option<JoinHandle<()>>>,

    /// `JoinHandle` for the stdin writer task.
    writer_handle: StdMutex<Option<JoinHandle<()>>>,

    /// `JoinHandle` for the child wait task.
    wait_handle: StdMutex<Option<JoinHandle<()>>>,

    /// Exit code for the process, when available.
    exit_code: std::sync::Arc<StdMutex<Option<i32>>>,

    /// Optional managed-network attempt guard. When present, dropping the session
    /// unregisters the attempt id so the network approval service does not leak
    /// state across turns.
    network_attempt_guard: Option<NetworkAttemptGuard>,
}

impl ExecCommandSession {
    pub(crate) fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        output_tx: broadcast::Sender<Vec<u8>>,
        initial_output_rx: broadcast::Receiver<Vec<u8>>,
        parts: ExecCommandSessionParts,
    ) -> (Self, broadcast::Receiver<Vec<u8>>) {
        let ExecCommandSessionParts {
            killer,
            #[cfg(unix)]
            process_group_id,
            #[cfg(target_os = "linux")]
            cgroup_pid,
            #[cfg(target_os = "linux")]
            memory_watchdog,
            reader_handle,
            writer_handle,
            wait_handle,
            exit_code,
            network_attempt_guard,
        } = parts;
        (
            Self {
                writer_tx,
                output_tx,
                #[cfg(unix)]
                process_group_id,
                #[cfg(target_os = "linux")]
                cgroup_pid,
                #[cfg(target_os = "linux")]
                memory_watchdog,
                killer: StdMutex::new(Some(killer)),
                reader_handle: StdMutex::new(Some(reader_handle)),
                writer_handle: StdMutex::new(Some(writer_handle)),
                wait_handle: StdMutex::new(Some(wait_handle)),
                exit_code,
                network_attempt_guard,
            },
            initial_output_rx,
        )
    }

    pub(crate) fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub(crate) fn output_receiver(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        match self.exit_code.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn exceeded_memory_limit(&self) -> Option<u64> {
        if let Some(memory_max_bytes) = self
            .memory_watchdog
            .as_ref()
            .filter(|watchdog| watchdog.limit_exceeded())
            .map(crate::cgroup::ExecMemoryWatchdog::memory_max_bytes)
        {
            return Some(memory_max_bytes);
        }

        let pid = self.cgroup_pid?;
        crate::cgroup::exec_cgroup_oom_killed(pid)
            .unwrap_or(false)
            .then(|| crate::cgroup::exec_cgroup_memory_max_bytes(pid))?
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn exceeded_memory_limit(&self) -> Option<u64> {
        None
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn exceeded_pids_limit(&self) -> Option<u64> {
        let pid = self.cgroup_pid?;
        crate::cgroup::exec_cgroup_pids_limit_hit(pid)
            .unwrap_or(false)
            .then(|| crate::cgroup::exec_cgroup_pids_max(pid))?
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn exceeded_pids_limit(&self) -> Option<u64> {
        None
    }
}

impl Drop for ExecCommandSession {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        drop(self.memory_watchdog.take());

        #[cfg(unix)]
        if let Some(process_group_id) = self.process_group_id.take() {
            let _ = crate::exec_command::process_group::kill_process_group(process_group_id);
        }

        #[cfg(target_os = "linux")]
        if let Some(pid) = self.cgroup_pid.take() {
            crate::cgroup::best_effort_cleanup_exec_cgroup(pid);
        }

        // Best-effort: terminate child first so blocking tasks can complete.
        if let Ok(mut killer_opt) = self.killer.lock()
            && let Some(mut killer) = killer_opt.take()
        {
            let _ = killer.kill();
        }

        // Abort background tasks; they may already have exited after kill.
        if let Ok(mut h) = self.reader_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut h) = self.writer_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut h) = self.wait_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }

        // Preserve the managed-network attempt guard for the lifetime of the session,
        // but explicitly drop it here so dead-code analysis sees it used.
        let _ = self.network_attempt_guard.take();
    }
}
