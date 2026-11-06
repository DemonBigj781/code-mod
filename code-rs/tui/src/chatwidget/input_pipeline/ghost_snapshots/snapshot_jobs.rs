fn create_workspace_ghost_commits(
    scope: &RepositoryScope,
) -> Result<Vec<(PathBuf, GhostCommit)>, GitToolingError> {
    let process_timeout = GHOST_SNAPSHOT_TIMEOUT.saturating_sub(Duration::from_secs(1));
    let deadline = Instant::now() + process_timeout;
    if scope.repositories.is_empty() {
        return Err(GitToolingError::NotAGitRepository {
            path: scope.root.clone(),
        });
    }
    let repo_paths = scope.repositories.clone();
    let mut snapshots = Vec::with_capacity(repo_paths.len());
    for repo_path in repo_paths {
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
            GitToolingError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "ghost snapshot workspace deadline exceeded",
            ))
        })?;
        let hook_repo = repo_path.clone();
        let commit = create_ghost_commit(
            &CreateGhostCommitOptions::new(repo_path.as_path())
                .command_timeout(remaining)
                .post_commit_hook(&move || bump_snapshot_epoch_for(&hook_repo)),
        )?;
        snapshots.push((repo_path, commit));
    }
    Ok(snapshots)
}

fn snapshot_scope_is_current(request_scope: &str, current_scope: &str) -> bool {
    request_scope == current_scope
}

fn queue_latest_snapshot<T>(
    queue: &mut VecDeque<(u64, T)>,
    next_job_id: u64,
    request: T,
) -> (u64, bool) {
    if let Some((job_id, queued_request)) = queue.back_mut() {
        *queued_request = request;
        return (*job_id, false);
    }
    queue.push_back((next_job_id, request));
    (next_job_id, true)
}

impl ChatWidget<'_> {
    pub(in super::super) fn capture_ghost_snapshot(&mut self, summary: Option<String>) -> GhostSnapshotJobHandle {
        if self.ghost_snapshots_disabled {
            return GhostSnapshotJobHandle::Skipped;
        }

        let scope = self.repository_scope();
        if let Some(error) = scope.error.as_deref() {
            self.push_background_tail(format!("Git snapshot skipped for {}: {error}", scope.label));
            return GhostSnapshotJobHandle::Skipped;
        }
        let request = GhostSnapshotRequest::new(
            summary,
            self.current_conversation_snapshot(),
            self.history_snapshot_for_persistence(),
            scope,
        );
        self.enqueue_ghost_snapshot(request)
    }

    pub(in super::super) fn capture_ghost_snapshot_blocking(&mut self, summary: Option<String>) -> Option<GhostSnapshot> {
        if self.ghost_snapshots_disabled {
            return None;
        }

        let scope = self.repository_scope();
        if let Some(error) = scope.error.as_deref() {
            self.push_background_tail(format!("Git snapshot skipped for {}: {error}", scope.label));
            return None;
        }
        let request = GhostSnapshotRequest::new(
            summary,
            self.current_conversation_snapshot(),
            self.history_snapshot_for_persistence(),
            scope,
        );
        let started_at = request.started_at;
        let result = create_workspace_ghost_commits(&request.scope);
        let elapsed = started_at.elapsed();
        
        self.finalize_ghost_snapshot(request, result, elapsed)
    }

    pub(in super::super) fn dispatch_queued_batch(&mut self, batch: Vec<UserMessage>) {
        let sender = self.code_op_tx.clone();
        self.dispatch_queued_batch_with_sender(batch, |op| {
            sender
                .send(op)
                .map_err(|error| {
                    let message = error.to_string();
                    (error.0, message)
                })
        });
    }

    pub(in super::super) fn dispatch_queued_batch_with_sender<F>(
        &mut self,
        batch: Vec<UserMessage>,
        mut send: F,
    ) where
        F: FnMut(Op) -> Result<(), (Op, String)>,
    {
        if batch.is_empty() {
            return;
        }

        let mut messages: Vec<UserMessage> = Vec::with_capacity(batch.len());

        for message in batch {
            let Some(message) = self.take_queued_user_message(&message) else {
                tracing::info!("Skipping queued user input removed before dispatch");
                continue;
            };
            messages.push(message);
        }

        if messages.is_empty() {
            return;
        }

        let mut combined_items: Vec<InputItem> = Vec::new();

        for (idx, message) in messages.iter().enumerate() {
            if idx > 0 && !combined_items.is_empty() && !message.ordered_items.is_empty() {
                combined_items.push(InputItem::Text {
                    text: "\n\n".to_owned(),
                });
            }
            combined_items.extend(message.ordered_items.clone());
        }

        let total_items = combined_items.len();
        let ephemeral_count = combined_items
            .iter()
            .filter(|item| matches!(item, InputItem::EphemeralImage { .. }))
            .count();
        if ephemeral_count > 0 {
            tracing::info!(
                "Sending {} items to model (including {} ephemeral images)",
                total_items,
                ephemeral_count
            );
        }

        let dispatch_result = if !combined_items.is_empty() {
            if !self.flush_pending_agent_notes_with_sender(&mut send) {
                Err("pending agent notes could not be sent".to_owned())
            } else {
                send(Op::UserInput {
                    items: combined_items,
                    final_output_json_schema: None,
                })
                .map_err(|(_, error)| error)
            }
        } else {
            Ok(())
        };

        match dispatch_result {
            Ok(()) => {
                for message in messages {
                    self.finalize_sent_user_message(message);
                }
            }
            Err(e) => {
                tracing::error!("failed to dispatch queued user input: {e}");
                for message in messages.into_iter().rev() {
                    self.queued_user_messages.push_front(message);
                }
                self.refresh_queued_user_messages(true);
            }
        }
    }

    pub(in super::super) fn dispatch_queued_user_message_now(&mut self, message: UserMessage) {
        let message = self.take_queued_user_message(&message).unwrap_or(message);
        let items = message.ordered_items.clone();
        tracing::info!(
            "[queue] Dispatching single queued message via coordinator (queue_remaining={})",
            self.queued_user_messages.len()
        );
        match self.code_op_tx.send(Op::QueueUserInput { items }) {
            Ok(()) => {
                self.finalize_sent_user_message(message);
            }
            Err(err) => {
                tracing::error!("failed to send QueueUserInput op: {err}");
                self.queued_user_messages.push_front(message);
                self.refresh_queued_user_messages(true);
            }
        }
    }

    pub(in super::super) fn dispatch_queued_batch_via_coordinator(&mut self, batch: Vec<UserMessage>) {
        if batch.is_empty() {
            return;
        }

        tracing::info!(
            "[queue] Draining batch via coordinator path (batch_size={}, auto_active={})",
            batch.len(),
            self.auto_state.is_active()
        );

        for message in batch {
            let Some(message) = self.take_queued_user_message(&message) else {
                tracing::info!("[queue] Skipping queued user input removed before dispatch");
                continue;
            };

            let items = message.ordered_items.clone();
            match self.code_op_tx.send(Op::QueueUserInput { items }) {
                Ok(()) => {
                    self.finalize_sent_user_message(message);
                }
                Err(err) => {
                    tracing::error!("[queue] Failed to send QueueUserInput op: {err}");
                    self.queued_user_messages.push_front(message);
                    self.refresh_queued_user_messages(true);
                    break;
                }
            }
        }
    }

    pub(in super::super) fn take_queued_user_message(&mut self, target: &UserMessage) -> Option<UserMessage> {
        let position = self
            .queued_user_messages
            .iter()
            .position(|message| message == target)?;
        let removed = self.queued_user_messages.remove(position)?;
        self.refresh_queued_user_messages(false);
        Some(removed)
    }

    pub(in super::super) fn enqueue_ghost_snapshot(&mut self, request: GhostSnapshotRequest) -> GhostSnapshotJobHandle {
        let (job_id, inserted) = queue_latest_snapshot(
            &mut self.ghost_snapshot_queue,
            self.next_ghost_snapshot_id,
            request,
        );
        if inserted {
            self.next_ghost_snapshot_id = self.next_ghost_snapshot_id.wrapping_add(1);
        }
        self.spawn_next_ghost_snapshot();
        GhostSnapshotJobHandle::Scheduled(job_id)
    }

    pub(in super::super) fn spawn_next_ghost_snapshot(&mut self) {
        if self.ghost_snapshots_disabled {
            self.ghost_snapshot_queue.clear();
            return;
        }
        if self.active_ghost_snapshot.is_some() {
            return;
        }
        let Some((job_id, request)) = self.ghost_snapshot_queue.pop_front() else {
            return;
        };

        let scope = request.scope.clone();
        let app_event_tx = self.app_event_tx.clone();
        let notice_ticket = self.make_background_tail_ticket();
        let started_at = request.started_at;
        self.active_ghost_snapshot = Some((job_id, request));

        tokio::spawn(async move {
            let worker_scope = scope.clone();
            let handle = tokio::task::spawn_blocking(move || {
                create_workspace_ghost_commits(&worker_scope)
            });
            tokio::pin!(handle);

            let mut notice_sent = false;
            let notice_sleep = tokio::time::sleep(GHOST_SNAPSHOT_NOTICE_THRESHOLD);
            tokio::pin!(notice_sleep);
            let timeout_sleep = tokio::time::sleep(GHOST_SNAPSHOT_TIMEOUT);
            tokio::pin!(timeout_sleep);

            let join_result = loop {
                tokio::select! {
                    res = &mut handle => break res,
                    _ = &mut timeout_sleep => {
                        handle.as_mut().abort();
                        let elapsed = started_at.elapsed();
                        let err = GitToolingError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "ghost snapshot exceeded {}",
                                format_duration(GHOST_SNAPSHOT_TIMEOUT)
                            ),
                        ));
                        let event = AppEvent::GhostSnapshotFinished {
                            job_id,
                            result: Err(err),
                            elapsed,
                        };
                        app_event_tx.send(event);
                        return;
                    }
                    _ = &mut notice_sleep, if !notice_sent => {
                        notice_sent = true;
                        let elapsed = started_at.elapsed();
                        let message = format!(
                            "Git snapshot still running for {} ({} repositories)… {} elapsed.",
                            scope.label,
                            scope.repositories.len(),
                            format_duration(elapsed),
                        );
                        app_event_tx.send_background_event_with_ticket(&notice_ticket, message);
                    }
                }
            };

            let elapsed = started_at.elapsed();
            let event = match join_result {
                Ok(Ok(repositories)) => AppEvent::GhostSnapshotFinished {
                    job_id,
                    result: Ok(repositories),
                    elapsed,
                },
                Ok(Err(err)) => AppEvent::GhostSnapshotFinished {
                    job_id,
                    result: Err(err),
                    elapsed,
                },
                Err(join_err) => {
                    let err = GitToolingError::Io(io::Error::other(
                        format!("ghost snapshot task failed: {join_err}"),
                    ));
                    AppEvent::GhostSnapshotFinished {
                        job_id,
                        result: Err(err),
                        elapsed,
                    }
                }
            };

            app_event_tx.send(event);
        });
    }

    pub(in super::super) fn finalize_ghost_snapshot(
        &mut self,
        request: GhostSnapshotRequest,
        result: Result<Vec<(PathBuf, GhostCommit)>, GitToolingError>,
        elapsed: Duration,
    ) -> Option<GhostSnapshot> {
        let current_scope = self.repository_scope();
        if !snapshot_scope_is_current(&request.scope.key, &current_scope.key) {
            self.push_background_tail(format!(
                "Discarded Git snapshot for previous scope {}; active scope is {}.",
                request.scope.label, current_scope.label
            ));
            return None;
        }
        match result {
            Ok(repositories) => {
                self.ghost_snapshots_disabled = false;
                self.ghost_snapshots_disabled_reason = None;
                let snapshot = GhostSnapshot::new(
                    repositories,
                    request.summary,
                    request.conversation,
                    request.history,
                );
                self.ghost_snapshots.push(snapshot.clone());
                session_log::log_history_snapshot(
                    snapshot.commit().id(),
                    snapshot.summary.as_deref(),
                    &snapshot.history,
                );
                if self.ghost_snapshots.len() > MAX_TRACKED_GHOST_COMMITS {
                    self.ghost_snapshots.remove(0);
                }
                if elapsed >= GHOST_SNAPSHOT_NOTICE_THRESHOLD {
                    self.push_background_tail(format!(
                        "Git snapshot captured for {} ({} repositories) in {}.",
                        request.scope.label,
                        snapshot.repositories.len(),
                        format_duration(elapsed),
                    ));
                }
                Some(snapshot)
            }
            Err(err) => {
                if let GitToolingError::Io(io_err) = &err
                    && io_err.kind() == io::ErrorKind::TimedOut {
                        self.ghost_snapshots_disabled = true;
                        self.ghost_snapshot_queue.clear();
                        let message = format!(
                            "Snapshots disabled for this session after Git timed out at {}. The timed-out Git process was terminated.",
                            format_duration(elapsed)
                        );
                        let hint = "Restart Code after reducing the workspace size or resolving the slow Git filter to re-enable snapshots.".to_owned();
                        self.ghost_snapshots_disabled_reason = Some(GhostSnapshotsDisabledReason {
                            message: message.clone(),
                            hint: Some(hint.clone()),
                        });
                        self.push_background_tail(message);
                        self.push_background_tail(hint);
                        tracing::warn!(
                            elapsed = %format_duration(elapsed),
                            "ghost snapshot timed out; snapshots disabled for this session"
                        );
                        return None;
                    }
                if matches!(err, GitToolingError::NotAGitRepository { .. }) {
                    self.ghost_snapshots_disabled = false;
                    self.ghost_snapshots_disabled_reason = None;
                    self.push_background_tail(
                        "No Git repositories found under the active workspace; snapshot skipped."
                            .to_owned(),
                    );
                    tracing::info!("ghost snapshot skipped because no repository was found");
                    return None;
                }

                self.ghost_snapshots_disabled = true;
                let (message, hint) = match &err {
                    _ => (
                        format!("Snapshots disabled after Git error: {err}"),
                        Some(
                            "Restart Code after resolving the issue to re-enable snapshots.".to_owned(),
                        ),
                    ),
                };
                self.ghost_snapshots_disabled_reason = Some(GhostSnapshotsDisabledReason {
                    message: message.clone(),
                    hint: hint.clone(),
                });
                self.push_background_tail(message);
                if let Some(hint) = hint {
                    self.push_background_tail(hint);
                }
                tracing::warn!("failed to create ghost snapshot: {err}");
                self.ghost_snapshot_queue.clear();
                None
            }
        }
    }
}

#[cfg(test)]
mod workspace_snapshot_tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn stale_snapshot_results_are_rejected_after_project_scope_changes() {
        assert!(!snapshot_scope_is_current("project:old", "project:new"));
        assert!(snapshot_scope_is_current("project:code", "project:code"));
    }

    fn init_repo(repo: &Path, contents: &str) {
        std::fs::create_dir_all(repo).expect("create repository directory");
        let init = Command::new("git")
            .current_dir(repo)
            .args(["init", "--initial-branch=main"])
            .status()
            .expect("run git init");
        assert!(init.success(), "git init failed for {}", repo.display());
        std::fs::write(repo.join("tracked.txt"), contents).expect("write tracked file");
        let commit = Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "user.name=Snapshot Test",
                "-c",
                "user.email=snapshot@example.com",
                "add",
                "tracked.txt",
            ])
            .status()
            .expect("run git add");
        assert!(commit.success(), "git add failed for {}", repo.display());
        let commit = Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "user.name=Snapshot Test",
                "-c",
                "user.email=snapshot@example.com",
                "commit",
                "-m",
                "initial",
            ])
            .status()
            .expect("run git commit");
        assert!(commit.success(), "git commit failed for {}", repo.display());
    }

    fn scope(root: PathBuf, repositories: Vec<PathBuf>) -> RepositoryScope {
        RepositoryScope {
            key: format!("test:{}", root.display()),
            label: "test scope".to_owned(),
            root,
            repositories,
            error: None,
        }
    }

    #[test]
    fn workspace_snapshot_uses_repository_containing_nested_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested_cwd = repo.join("src").join("nested");
        init_repo(&repo, "before\n");
        std::fs::create_dir_all(&nested_cwd).expect("create nested cwd");
        std::fs::write(repo.join("tracked.txt"), "after\n").expect("modify tracked file");

        let snapshots = create_workspace_ghost_commits(&scope(
            nested_cwd,
            vec![repo.canonicalize().expect("canonical repository")],
        ))
        .expect("capture containing repository");

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].0, repo.canonicalize().expect("canonical repository"));
        assert!(!snapshots[0].1.id().is_empty());
    }

    #[test]
    fn workspace_snapshot_captures_all_repositories_below_non_git_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let repo_b = workspace.join("b-repo");
        let repo_a = workspace.join("a-repo");
        init_repo(&repo_b, "before b\n");
        init_repo(&repo_a, "before a\n");
        std::fs::write(repo_a.join("tracked.txt"), "after a\n").expect("modify first repo");
        std::fs::write(repo_b.join("tracked.txt"), "after b\n").expect("modify second repo");

        let snapshots = create_workspace_ghost_commits(&scope(
            workspace,
            vec![
                repo_a.canonicalize().expect("canonical first repository"),
                repo_b.canonicalize().expect("canonical second repository"),
            ],
        ))
        .expect("capture nested repositories");

        let repository_paths: Vec<_> = snapshots.into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            repository_paths,
            vec![
                repo_a.canonicalize().expect("canonical first repository"),
                repo_b.canonicalize().expect("canonical second repository"),
            ]
        );
    }

    #[test]
    fn workspace_snapshot_reports_no_repository_without_disabling_discovery() {
        let temp = tempfile::tempdir().expect("tempdir");

        let err = create_workspace_ghost_commits(&scope(
            temp.path().to_path_buf(),
            Vec::new(),
        ))
        .expect_err("no repository");

        assert!(matches!(err, GitToolingError::NotAGitRepository { .. }));
    }

    #[test]
    fn pending_snapshot_requests_are_coalesced_to_the_latest_request() {
        let mut queue = VecDeque::new();
        assert_eq!(queue_latest_snapshot(&mut queue, 7, "first"), (7, true));
        assert_eq!(queue_latest_snapshot(&mut queue, 8, "latest"), (7, false));
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![(7, "latest")]);
    }
}
