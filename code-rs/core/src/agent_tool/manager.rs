use super::*;

const MAX_RUNNING_SUBAGENTS: usize = 4;

// Agent status enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

// Agent information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    #[serde(default)]
    pub owner_session_id: Option<Uuid>,
    pub batch_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub name: Option<String>,
    pub prompt: String,
    pub context: Option<String>,
    pub output_goal: Option<String>,
    pub files: Vec<String>,
    pub read_only: bool,
    pub status: AgentStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub progress: Vec<String>,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    #[serde(default)]
    pub worktree_base: Option<String>,
    #[serde(default)]
    pub source_kind: Option<AgentSourceKind>,
    #[serde(skip)]
    pub log_tag: Option<String>,
    pub reasoning_effort: code_protocol::config_types::ReasoningEffort,
    #[serde(skip)]
    pub last_activity: DateTime<Utc>,
}

// Global agent manager
lazy_static::lazy_static! {
    pub static ref AGENT_MANAGER: Arc<RwLock<AgentManager>> = Arc::new(RwLock::new(AgentManager::new()));
}

pub struct AgentManager {
    agents: HashMap<String, Agent>,
    handles: HashMap<String, JoinHandle<()>>,
    // The process-wide AGENT_MANAGER shares these slots across all session tabs.
    worker_slots: Arc<tokio::sync::Semaphore>,
    event_senders: HashMap<Uuid, mpsc::UnboundedSender<AgentStatusUpdatePayload>>,
    debug_log_root: Option<PathBuf>,
    watchdog_handle: Option<JoinHandle<()>>,
    inactivity_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct AgentStatusUpdatePayload {
    pub agents: Vec<AgentInfo>,
    pub context: Option<String>,
    pub task: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AgentCreateRequest {
    pub owner_session_id: Option<Uuid>,
    pub model: String,
    pub name: Option<String>,
    pub prompt: String,
    pub context: Option<String>,
    pub output_goal: Option<String>,
    pub files: Vec<String>,
    pub read_only: bool,
    pub batch_id: Option<String>,
    pub config: Option<AgentConfig>,
    pub worktree_branch: Option<String>,
    pub worktree_base: Option<String>,
    pub source_kind: Option<AgentSourceKind>,
    pub reasoning_effort: code_protocol::config_types::ReasoningEffort,
}

#[cfg(test)]
mod tab_isolation_tests {
    use super::*;

    fn agent(owner: Uuid, name: &str) -> Agent {
        Agent {
            id: name.into(), owner_session_id: Some(owner), batch_id: Some("shared-label".into()),
            model: "test".into(), name: None, prompt: format!("{name}-task"),
            context: Some(format!("{name}-context")), output_goal: None, files: vec![], read_only: true,
            status: AgentStatus::Running, result: None, error: None,
            created_at: Utc::now(), started_at: None, completed_at: None, progress: vec![],
            worktree_path: None, branch_name: None, worktree_base: None, source_kind: None,
            log_tag: None, reasoning_effort: Default::default(), last_activity: Utc::now(),
        }
    }

    #[test]
    fn agent_id_lookup_and_unfiltered_listing_preserve_session_ownership() {
        let mut manager = AgentManager::new();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        manager.agents.insert("alpha".into(), agent(first, "alpha"));
        manager.agents.insert("bravo".into(), agent(second, "bravo"));
        for (owner, own, other) in [(first, "alpha", "bravo"), (second, "bravo", "alpha")] {
            assert!(manager.session_agent(owner, own, None).is_some());
            assert!(manager.session_agent(owner, other, None).is_none());
            assert!(manager.session_agent(owner, own, Some("wrong-batch")).is_none());
            assert!(manager.session_agent(owner, "missing", None).is_none());
            let listed = manager.list_session_agents(owner, None, None, false);
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].id, own);
        }
        manager.agents.get_mut("alpha").unwrap().batch_id = None;
        assert!(manager.session_agent(first, "alpha", None).is_some());
    }

    #[tokio::test]
    async fn tab_isolation_agent_updates_and_cancellation_follow_the_owner() {
        let mut manager = AgentManager::new();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let (first_tx, mut first_rx) = mpsc::unbounded_channel();
        let (second_tx, mut second_rx) = mpsc::unbounded_channel();
        manager.event_senders.insert(first, first_tx);
        manager.event_senders.insert(second, second_tx);
        manager.agents.insert("alpha".into(), agent(first, "alpha"));
        manager.agents.insert("bravo".into(), agent(second, "bravo"));
        for id in ["alpha", "bravo"] {
            manager.handles.insert(id.into(), tokio::spawn(std::future::pending()));
        }
        manager.send_agent_status_update();
        for (receiver, name) in [(&mut first_rx, "alpha"), (&mut second_rx, "bravo")] {
            let payload = receiver.try_recv().unwrap();
            assert_eq!(payload.agents.len(), 1);
            assert_eq!(payload.agents[0].id, name);
            assert_eq!(payload.context, Some(format!("{name}-context")));
            assert_eq!(payload.task, Some(format!("{name}-task")));
        }
        assert_eq!(manager.cancel_session_batch(first, "shared-label").await, 1);
        assert_eq!(manager.get_agent("bravo").unwrap().status, AgentStatus::Running);
        assert_eq!(manager.cancel_session_batch(second, "shared-label").await, 1);
        assert_eq!(manager.get_agent("alpha").unwrap().status, AgentStatus::Cancelled);
    }
}

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            handles: HashMap::new(),
            worker_slots: Arc::new(tokio::sync::Semaphore::new(MAX_RUNNING_SUBAGENTS)),
            event_senders: HashMap::new(),
            debug_log_root: None,
            watchdog_handle: None,
            inactivity_timeout: Duration::minutes(30),
        }
    }

    pub fn set_event_sender(&mut self, owner: Uuid, sender: mpsc::UnboundedSender<AgentStatusUpdatePayload>) {
        self.event_senders.retain(|_, sender| !sender.is_closed());
        self.event_senders.insert(owner, sender);
        self.start_watchdog();
    }

    pub fn remove_event_sender(&mut self, owner: Uuid) {
        self.event_senders.remove(&owner);
    }

    pub async fn cancel_session_batch(&mut self, owner: Uuid, batch: &str) -> usize {
        let ids: Vec<_> = self.agents.values()
            .filter(|agent| agent.owner_session_id == Some(owner) && agent.batch_id.as_deref() == Some(batch))
            .map(|agent| agent.id.clone()).collect();
        ids.iter().filter(|id| self.cancel_agent(id)).count()
    }

    fn start_watchdog(&mut self) {
        if self.watchdog_handle.is_some() {
            return;
        }

        let timeout = self.inactivity_timeout;
        let manager = Arc::downgrade(&AGENT_MANAGER);
        self.watchdog_handle = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TokioDuration::from_secs(60));
            loop {
                ticker.tick().await;

                let Some(manager_arc) = manager.upgrade() else { break; };

                let mut mgr = manager_arc.write().await;
                let now = Utc::now();
                let timeout_ids: Vec<String> = mgr
                    .agents
                    .iter()
                    // Time queued for a process slot is not worker inactivity.
                    .filter(|(_, agent)| agent.status == AgentStatus::Running)
                    .filter(|(_, agent)| now - agent.last_activity > timeout)
                    .map(|(id, _)| id.clone())
                    .collect();

                if timeout_ids.is_empty() {
                    continue;
                }

                for agent_id in &timeout_ids {
                    if let Some(handle) = mgr.handles.remove(agent_id) {
                        handle.abort();
                    }
                    if let Some(agent) = mgr.agents.get_mut(agent_id) {
                        agent.status = AgentStatus::Failed;
                        agent.error = Some(format!(
                            "Agent timed out after {} minutes of inactivity.",
                            timeout.num_minutes()
                        ));
                        agent.completed_at = Some(now);
                        Self::record_activity(agent);
                    }
                }

                // Notify listeners once per sweep.
                mgr.send_agent_status_update();
            }
        }));
    }

    pub fn set_debug_log_root(&mut self, root: Option<PathBuf>) {
        self.debug_log_root = root;
    }

    pub(crate) async fn touch_agent(agent_id: &str) {
        if let Some(manager) = Arc::downgrade(&AGENT_MANAGER).upgrade() {
            let mut mgr = manager.write().await;
            if let Some(agent) = mgr.agents.get_mut(agent_id) {
                Self::record_activity(agent);
            }
        }
    }

    fn record_activity(agent: &mut Agent) {
        agent.last_activity = Utc::now();
    }

    fn append_agent_log(&self, log_tag: &str, line: &str) {
        let Some(root) = &self.debug_log_root else { return; };
        let dir = root.join(log_tag);
        if let Err(err) = fs::create_dir_all(&dir) {
            warn!("failed to create agent log dir {:?}: {}", dir, err);
            return;
        }

        let file = dir.join("progress.log");
        match OpenOptions::new().create(true).append(true).open(&file) {
            Ok(mut fh) => {
                if let Err(err) = writeln!(fh, "{line}") {
                    warn!("failed to write agent log {:?}: {}", file, err);
                }
            }
            Err(err) => warn!("failed to open agent log {:?}: {}", file, err),
        }
    }

    fn send_agent_status_update(&self) {
        for (owner, sender) in &self.event_senders {
            let now = Utc::now();
            let agents: Vec<AgentInfo> = self
                .agents
                .values()
                .filter(|agent| agent.owner_session_id == Some(*owner))
                .map(|agent| {
                    // Just show the model name - status provides the useful info
                    let name = agent
                        .name.clone()
                        .unwrap_or_else(|| agent.model.clone());
                    let start = agent.started_at.unwrap_or(agent.created_at);
                    let end = agent.completed_at.unwrap_or(now);
                    let elapsed_ms = match end.signed_duration_since(start).num_milliseconds() {
                        value if value >= 0 => Some(value as u64),
                        _ => None,
                    };

                    AgentInfo {
                        id: agent.id.clone(),
                        name,
                        status: format!("{:?}", agent.status).to_lowercase(),
                        batch_id: agent.batch_id.clone(),
                        model: Some(agent.model.clone()),
                        last_progress: agent.progress.last().cloned(),
                        result: agent.result.clone(),
                        error: agent.error.clone(),
                        elapsed_ms,
                        token_count: None,
                        last_activity_at: match agent.status {
                            AgentStatus::Pending | AgentStatus::Running => {
                                Some(agent.last_activity.to_rfc3339())
                            }
                            _ => None,
                        },
                        seconds_since_last_activity: match agent.status {
                            AgentStatus::Pending | AgentStatus::Running => Some(
                                Utc::now()
                                    .signed_duration_since(agent.last_activity)
                                    .num_seconds()
                                    .max(0) as u64,
                            ),
                            _ => None,
                        },
                        source_kind: agent.source_kind.clone(),
                    }
                })
                .collect();

            // Get context and task from the first agent (they're all the same)
            let (context, task) = self
                .agents
                .values()
                .filter(|agent| agent.owner_session_id == Some(*owner))
                .next()
                .map_or((None, None), |agent| {
                    let context = agent
                        .context
                        .as_ref()
                        .filter(|value| !value.trim().is_empty())
                        .cloned();
                    let task = if agent.prompt.trim().is_empty() {
                        None
                    } else {
                        Some(agent.prompt.clone())
                    };
                    (context, task)
                });
            let payload = AgentStatusUpdatePayload { agents, context, task };
            let _ = sender.send(payload);
        }
    }

    pub async fn create_agent(
        &mut self,
        request: AgentCreateRequest,
    ) -> String {
        self.create_agent_internal(request).await
    }

    pub async fn create_agent_with_config(
        &mut self,
        mut request: AgentCreateRequest,
        config: AgentConfig,
    ) -> String {
        request.config = Some(config);
        self.create_agent_internal(request).await
    }

    pub async fn create_agent_with_options(
        &mut self,
        request: AgentCreateRequest,
    ) -> String {
        self.create_agent_internal(request).await
    }

    async fn create_agent_internal(
        &mut self,
        request: AgentCreateRequest,
    ) -> String {
        let AgentCreateRequest {
            owner_session_id,
            model,
            name,
            prompt,
            context,
            output_goal,
            files,
            read_only,
            batch_id,
            config,
            worktree_branch,
            worktree_base,
            source_kind,
            reasoning_effort,
        } = request;
        let agent_id = Uuid::new_v4().to_string();

        let log_tag = match source_kind {
            Some(AgentSourceKind::AutoReview) => {
                Some(format!("agents/auto-review/{agent_id}"))
            }
            _ => None,
        };

        let agent = Agent {
            id: agent_id.clone(),
            owner_session_id,
            batch_id,
            model,
            name: normalize_agent_name(name),
            prompt,
            context,
            output_goal,
            files,
            read_only,
            status: AgentStatus::Pending,
            result: None,
            error: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            progress: Vec::new(),
            worktree_path: None,
            branch_name: worktree_branch,
            worktree_base,
            source_kind,
            log_tag,
            reasoning_effort,
            last_activity: Utc::now(),
        };

        self.agents.insert(agent_id.clone(), agent);

        // Send initial status update
        self.send_agent_status_update();

        // Spawn async agent
        let agent_id_clone = agent_id.clone();
        let worker_slots = Arc::clone(&self.worker_slots);
        let handle = tokio::spawn(async move {
            run_with_process_slot(worker_slots, async move {
                execute_agent(agent_id_clone, config).await;
            })
            .await;
        });

        self.handles.insert(agent_id.clone(), handle);

        agent_id
    }

    pub fn get_agent(&self, agent_id: &str) -> Option<Agent> {
        self.agents.get(agent_id).cloned()
    }

    pub fn session_agent(&self, owner: Uuid, agent_id: &str, batch: Option<&str>) -> Option<&Agent> {
        self.agents.get(agent_id).filter(|agent| {
            agent.owner_session_id == Some(owner)
                && batch.is_none_or(|batch| agent.batch_id.as_deref() == Some(batch))
        })
    }

    pub fn get_all_agents(&self) -> impl Iterator<Item = &Agent> {
        self.agents.values()
    }

    pub fn list_agents(
        &self,
        status_filter: Option<AgentStatus>,
        batch_id: Option<String>,
        recent_only: bool,
    ) -> Vec<Agent> {
        self.list_agents_scoped(None, status_filter, batch_id, recent_only)
    }

    pub fn list_session_agents(&self, owner: Uuid, status: Option<AgentStatus>, batch: Option<String>, recent: bool) -> Vec<Agent> {
        self.list_agents_scoped(Some(owner), status, batch, recent)
    }

    fn list_agents_scoped(&self, owner: Option<Uuid>, status_filter: Option<AgentStatus>, batch_id: Option<String>, recent_only: bool) -> Vec<Agent> {
        let cutoff = recent_only.then(|| Utc::now() - Duration::hours(2));

        self.agents
            .values()
            .filter(|agent| {
                if owner.is_some_and(|owner| agent.owner_session_id != Some(owner)) { return false; }
                if let Some(ref filter) = status_filter
                    && agent.status != *filter {
                        return false;
                    }
                if let Some(ref batch) = batch_id
                    && agent.batch_id.as_ref() != Some(batch) {
                        return false;
                    }
                if let Some(cutoff) = cutoff
                    && agent.created_at < cutoff {
                        return false;
                    }
                true
            })
            .cloned()
            .collect()
    }

    pub fn has_active_agents(&self) -> bool {
        self.agents
            .values()
            .any(|agent| matches!(agent.status, AgentStatus::Pending | AgentStatus::Running))
    }

    pub fn cancel_agent(&mut self, agent_id: &str) -> bool {
        if !self.agents.get(agent_id).is_some_and(|agent| {
            matches!(agent.status, AgentStatus::Pending | AgentStatus::Running)
        }) {
            return false;
        }
        if let Some(handle) = self.handles.remove(agent_id) {
            handle.abort();
            if let Some(agent) = self.agents.get_mut(agent_id) {
                agent.status = AgentStatus::Cancelled;
                agent.completed_at = Some(Utc::now());
            }
            self.send_agent_status_update();
            true
        } else {
            false
        }
    }

    pub async fn cancel_batch(&mut self, batch_id: &str) -> usize {
        let agent_ids: Vec<String> = self
            .agents
            .values()
            .filter(|agent| agent.batch_id.as_ref() == Some(&batch_id.to_owned()))
            .map(|agent| agent.id.clone())
            .collect();

        let mut count = 0;
        for agent_id in agent_ids {
            if self.cancel_agent(&agent_id) {
                count += 1;
            }
        }
        count
    }

    pub async fn update_agent_status(&mut self, agent_id: &str, status: AgentStatus) {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = status;
            if agent.status == AgentStatus::Running && agent.started_at.is_none() {
                agent.started_at = Some(Utc::now());
            }
            if matches!(
                agent.status,
                AgentStatus::Completed | AgentStatus::Failed | AgentStatus::Cancelled
            ) {
                agent.completed_at = Some(Utc::now());
            }
            Self::record_activity(agent);
            // Send status update event
            self.send_agent_status_update();
        }
    }

    pub async fn update_agent_result(&mut self, agent_id: &str, result: Result<String, String>) {
        let debug_enabled = self.debug_log_root.is_some();

        if let Some((log_tag, log_lines)) = self.agents.get_mut(agent_id).map(|agent| {
            let log_tag = if debug_enabled { agent.log_tag.clone() } else { None };

            let mut log_lines: Vec<String> = Vec::new();
            if debug_enabled {
                let stamp = Utc::now().format("%H:%M:%S");
                match &result {
                    Ok(output) => {
                        log_lines.push(format!("{stamp}: [result] completed"));
                        if !output.trim().is_empty() {
                            log_lines.push(output.trim_end().to_owned());
                        }
                    }
                    Err(error) => {
                        log_lines.push(format!("{stamp}: [result] failed"));
                        log_lines.push(error.clone());
                    }
                }
            }

            match result {
                Ok(output) => {
                    agent.result = Some(output);
                    agent.status = AgentStatus::Completed;
                }
                Err(error) => {
                    agent.error = Some(error);
                    agent.status = AgentStatus::Failed;
                }
            }
            agent.completed_at = Some(Utc::now());
            Self::record_activity(agent);

            (log_tag, log_lines)
        }) {
            if let Some(tag) = log_tag {
                for line in log_lines {
                    self.append_agent_log(&tag, &line);
                }
            }
            // Send status update event
            self.send_agent_status_update();
        }
    }

    pub async fn add_progress(&mut self, agent_id: &str, message: String) {
        let debug_enabled = self.debug_log_root.is_some();

        if let Some((log_tag, entry)) = self.agents.get_mut(agent_id).map(|agent| {
            let entry = format!("{}: {}", Utc::now().format("%H:%M:%S"), message);
            let log_tag = if debug_enabled { agent.log_tag.clone() } else { None };
            agent.progress.push(entry.clone());
            Self::record_activity(agent);
            (log_tag, entry)
        }) {
            if let Some(tag) = log_tag {
                self.append_agent_log(&tag, &entry);
            }
            // Send updated agent status with the latest progress
            self.send_agent_status_update();
        }
    }

    pub fn update_worktree_info(
        &mut self,
        agent_id: &str,
        worktree_path: String,
        branch_name: String,
    ) {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.worktree_path = Some(worktree_path);
            agent.branch_name = Some(branch_name);
        }
    }
}

async fn run_with_process_slot<F>(worker_slots: Arc<tokio::sync::Semaphore>, process: F)
where
    F: std::future::Future<Output = ()> + Send,
{
    let Ok(_permit) = worker_slots.acquire_owned().await else {
        return;
    };
    process.await;
}

#[cfg(test)]
mod process_slot_tests {
    use super::{MAX_RUNNING_SUBAGENTS, run_with_process_slot};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    #[tokio::test]
    async fn four_process_slots_are_shared_and_cancellation_releases_one() {
        let slots = Arc::new(Semaphore::new(MAX_RUNNING_SUBAGENTS));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let mut workers = Vec::new();

        for index in 0..=MAX_RUNNING_SUBAGENTS {
            let slots = Arc::clone(&slots);
            let started_tx = started_tx.clone();
            workers.push(tokio::spawn(run_with_process_slot(slots, async move {
                started_tx.send(index).expect("test receiver remains open");
                std::future::pending::<()>().await;
            })));
        }
        drop(started_tx);

        let mut running = Vec::new();
        for _ in 0..MAX_RUNNING_SUBAGENTS {
            running.push(
                timeout(Duration::from_secs(2), started_rx.recv())
                    .await
                    .expect("worker should start")
                    .expect("worker channel remains open"),
            );
        }
        assert!(
            timeout(Duration::from_millis(50), started_rx.recv())
                .await
                .is_err()
        );

        workers[running[0]].abort();
        let next = timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .expect("queued worker should start when a running worker is canceled")
            .expect("worker channel remains open");
        assert!(!running.contains(&next));

        for worker in workers {
            worker.abort();
            let _ = worker.await;
        }
    }
}
