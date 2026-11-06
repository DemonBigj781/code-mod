use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoReviewTarget {
    IsolatedRepository(PathBuf),
    Workspace { repository_roots: Vec<PathBuf> },
}

fn resolve_auto_review_target(scope: &RepositoryScope) -> Result<AutoReviewTarget, String> {
    if let Some(error) = scope.error.as_deref() {
        return Err(error.to_owned());
    }
    match scope.repositories.as_slice() {
        [] => Err(format!(
            "No Git repositories are present in the active project scope {}.",
            scope.label
        )),
        [repository] => Ok(AutoReviewTarget::IsolatedRepository(repository.clone())),
        repositories => Ok(AutoReviewTarget::Workspace {
            repository_roots: repositories.to_vec(),
        }),
    }
}

enum PreparedAutoReview {
    Isolated {
        worktree_path: PathBuf,
        branch: String,
        snapshot_id: String,
        worktree_guard: code_core::review_coord::ReviewGuard,
    },
    Workspace {
        repository_roots: Vec<PathBuf>,
    },
}

async fn prepare_isolated_auto_review(
    repo_path: &Path,
    base_snapshot: Option<&GhostCommit>,
    prefer_fallback: bool,
) -> Result<PreparedAutoReview, String> {
    let _ = code_core::review_coord::clear_stale_lock_if_dead(Some(repo_path));
    let snapshot = task::spawn_blocking({
        let repo_path = repo_path.to_path_buf();
        let base_snapshot = base_snapshot.cloned();
        move || {
            let mut options = CreateGhostCommitOptions::new(repo_path.as_path())
                .message("auto review snapshot");
            if let Some(base) = base_snapshot.as_ref() {
                options = options.parent(base.id());
            }
            let hook_repo = repo_path.clone();
            let hook = move || bump_snapshot_epoch_for(&hook_repo);
            create_ghost_commit(&options.post_commit_hook(&hook))
        }
    })
    .await
    .map_err(|err| format!("failed to spawn snapshot task: {err}"))?
    .map_err(|err| format!("failed to capture snapshot: {err}"))?;

    let snapshot_id = snapshot.id().to_owned();
    bump_snapshot_epoch_for(repo_path);

    let (worktree_path, branch, worktree_guard) = if prefer_fallback {
        allocate_fallback_auto_review_worktree(repo_path, &snapshot_id).await?
    } else {
        match try_acquire_lock("review", repo_path) {
            Ok(Some(guard)) => {
                let path = code_core::git_worktree::prepare_reusable_worktree(
                    repo_path,
                    AUTO_REVIEW_SHARED_WORKTREE,
                    snapshot_id.as_str(),
                    true,
                )
                .await
                .map_err(|err| format!("failed to prepare worktree: {err}"))?;
                (path, AUTO_REVIEW_SHARED_WORKTREE.to_owned(), guard)
            }
            Ok(None) => allocate_fallback_auto_review_worktree(repo_path, &snapshot_id).await?,
            Err(err) => return Err(format!("could not acquire review lock: {err}")),
        }
    };

    Ok(PreparedAutoReview::Isolated {
        worktree_path,
        branch,
        snapshot_id,
        worktree_guard,
    })
}

fn workspace_review_prompt(
    workspace_path: &Path,
    repository_roots: &[PathBuf],
    scope_error: Option<&str>,
    turn_context: Option<&str>,
) -> String {
    let scope = if repository_roots.is_empty() {
        format!("workspace {}", workspace_path.display())
    } else {
        let roots = repository_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("workspace {} and repositories: {roots}", workspace_path.display())
    };
    let mut prompt = format!(
        "Review the current changes in {scope}. Work read-only. Git metadata may be absent, malformed, detached, conflicted, or otherwise unusable; do not fail or stop for that reason. Inspect the files directly and use any healthy repository metadata only as optional evidence. Identify critical bugs, regressions, security/performance/concurrency risks, or incorrect assumptions. Provide actionable findings with file references; ignore minor style or formatting nits."
    );
    if let Some(error) = scope_error {
        prompt.push_str("\n\nProject scope warning: ");
        prompt.push_str(error);
        prompt.push_str(" Do not expand the review beyond the declared project path.");
    }
    if let Some(context) = turn_context {
        prompt.push_str("\n\n");
        prompt.push_str(context);
    }
    prompt
}

pub(super) async fn run_background_review_inner(
    session_id: Option<uuid::Uuid>,
    config: Config,
    app_event_tx: AppEventSender,
    base_snapshot: Option<GhostCommit>,
    turn_context: Option<String>,
    prefer_fallback: bool,
    repository_scope: RepositoryScope,
) {
    let target = match resolve_auto_review_target(&repository_scope) {
        Ok(target) => target,
        Err(error) => {
            app_event_tx.send(AppEvent::BackgroundReviewFinished {
                worktree_path: std::path::PathBuf::new(),
                branch: String::new(),
                has_findings: false,
                findings: 0,
                summary: Some(format!(
                    "Auto review skipped for {}: {error}",
                    repository_scope.label
                )),
                error: None,
                agent_id: None,
                snapshot: None,
            });
            return;
        }
    };

    // Best-effort: clean up any stale lock left by a cancelled review process.
    let _ = code_core::review_coord::clear_stale_lock_if_dead(Some(&repository_scope.root));

    // Prevent duplicate auto-reviews within this process: if any AutoReview agent
    // is already pending/running, bail early with a benign notice.
    {
        let mgr = code_core::AGENT_MANAGER.read().await;
        let busy = mgr
            .list_agents(None, Some("auto-review".to_owned()), false)
            .into_iter()
            .any(|agent| {
                let status = format!("{:?}", agent.status).to_ascii_lowercase();
                status == "running" || status == "pending"
            });
        if busy {
            app_event_tx.send(AppEvent::BackgroundReviewFinished {
                worktree_path: std::path::PathBuf::new(),
                branch: String::new(),
                has_findings: false,
                findings: 0,
                summary: Some("Auto review skipped: another auto review is already running.".to_owned()),
                error: None,
                agent_id: None,
                snapshot: None,
            });
            return;
        }
    }

    let app_event_tx_clone = app_event_tx.clone();
    let outcome = async move {
        // Ensure Codex models are invoked via the `code-` CLI shim so they exist on PATH.
        fn ensure_code_prefix(model: &str) -> String {
            let lower = model.to_ascii_lowercase();
            if lower.starts_with("code-") {
                model.to_owned()
            } else {
                format!("code-{model}")
            }
        }

        let review_model = ensure_code_prefix(&config.auto_review_model);

        // Allow the spawned agent to reuse the parent's review lock without blocking.
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        env.insert("CODE_REVIEW_LOCK_LEASE".to_owned(), "1".to_owned());
        let mut agent_config = code_core::agent_defaults::agent_config_for_model(
            &config.agents,
            &review_model,
        )
        .cloned()
        .unwrap_or_else(|| code_core::config_types::AgentConfig {
            name: review_model.clone(),
            command: String::new(),
            args: Vec::new(),
            read_only: false,
            enabled: false,
            session_enabled: true,
            review_enabled: true,
            auto_drive_enabled: true,
            description: None,
            env: None,
            args_read_only: None,
            args_write: None,
            instructions: None,
        });
        if !agent_config.review_enabled {
            return Err(format!(
                "auto review model '{}' is disabled for review",
                config.auto_review_model
            ));
        }
        agent_config.env.get_or_insert_with(Default::default).extend(env);

        let prepared = match target {
            AutoReviewTarget::IsolatedRepository(repo_path) => {
                match prepare_isolated_auto_review(
                    &repo_path,
                    base_snapshot.as_ref(),
                    prefer_fallback,
                )
                .await
                {
                    Ok(prepared) => prepared,
                    Err(err) => {
                        tracing::warn!(
                            repository = %repo_path.display(),
                            "auto review Git isolation unavailable; continuing in read-only workspace mode: {err}"
                        );
                        PreparedAutoReview::Workspace {
                            repository_roots: vec![repo_path],
                        }
                    }
                }
            }
            AutoReviewTarget::Workspace { repository_roots } => {
                PreparedAutoReview::Workspace { repository_roots }
            }
        };

        let (
            worktree_path,
            branch,
            snapshot_id,
            review_prompt,
            read_only,
            working_dir,
            files,
            worktree_branch,
            worktree_base,
            worktree_guard,
        ) = match prepared {
            PreparedAutoReview::Isolated {
                worktree_path,
                branch,
                snapshot_id,
                worktree_guard,
            } => {
                let mut prompt = format!(
                    "/review Analyze only changes made in commit {snapshot_id}. Identify critical bugs, regressions, security/performance/concurrency risks or incorrect assumptions. Provide actionable feedback and references to the changed code; ignore minor style or formatting nits."
                );
                if let Some(context) = turn_context.as_deref() {
                    prompt.push_str("\n\n");
                    prompt.push_str(context);
                }
                (
                    worktree_path,
                    branch.clone(),
                    Some(snapshot_id.clone()),
                    prompt,
                    false,
                    None,
                    Vec::new(),
                    Some(branch),
                    Some(snapshot_id),
                    Some(worktree_guard),
                )
            }
            PreparedAutoReview::Workspace { repository_roots } => {
                let prompt = workspace_review_prompt(
                    &repository_scope.root,
                    &repository_roots,
                    repository_scope.error.as_deref(),
                    turn_context.as_deref(),
                );
                let files = if repository_roots.is_empty() {
                    vec![repository_scope.root.display().to_string()]
                } else {
                    repository_roots
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect()
                };
                (
                    repository_scope.root.clone(),
                    String::new(),
                    None,
                    prompt,
                    true,
                    Some(repository_scope.root.clone()),
                    files,
                    None,
                    None,
                    None,
                )
            }
        };

        let mut manager = code_core::AGENT_MANAGER.write().await;
        let agent_id = manager
            .create_agent_with_options(code_core::AgentCreateRequest {
                owner_session_id: session_id,
                model: review_model,
                name: Some("Auto Review".to_owned()),
                prompt: review_prompt,
                context: None,
                output_goal: None,
                files,
                read_only,
                working_dir,
                batch_id: Some(if branch.is_empty() {
                    "auto-review-workspace".to_owned()
                } else {
                    branch.clone()
                }),
                config: Some(agent_config.clone()),
                worktree_branch,
                worktree_base,
                source_kind: Some(code_core::protocol::AgentSourceKind::AutoReview),
                reasoning_effort: config.auto_review_model_reasoning_effort.into(),
            })
            .await;
        if let Some(worktree_guard) = worktree_guard {
            insert_background_lock(&agent_id, worktree_guard);
        }
        drop(manager);

        app_event_tx_clone.send(AppEvent::BackgroundReviewStarted {
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            agent_id: Some(agent_id.clone()),
            snapshot: snapshot_id.clone(),
        });
        Ok::<(), String>(())
    }
    .await;

    if let Err(err) = outcome {
        app_event_tx.send(AppEvent::BackgroundReviewFinished {
            worktree_path: std::path::PathBuf::new(),
            branch: String::new(),
            has_findings: false,
            findings: 0,
            summary: None,
            error: Some(err),
            agent_id: None,
            snapshot: None,
        });
    }
}

pub(super) fn insert_background_lock_inner(
    agent_id: &str,
    guard: code_core::review_coord::ReviewGuard,
) {
    if let Ok(mut map) = BACKGROUND_REVIEW_LOCKS.lock() {
        map.insert(agent_id.to_owned(), guard);
    }
}

pub(super) fn release_background_lock_inner(agent_id: Option<&String>) {
    if let Some(id) = agent_id
        && let Ok(mut map) = BACKGROUND_REVIEW_LOCKS.lock() {
            map.remove(id);
    }
}

#[cfg(test)]
mod repository_mode_tests {
    use super::*;
    use std::process::Command;

    fn init_repo(repo: &Path) {
        std::fs::create_dir_all(repo).expect("create repository directory");
        let status = Command::new("git")
            .current_dir(repo)
            .args(["init", "--initial-branch=main"])
            .status()
            .expect("run git init");
        assert!(status.success(), "git init failed for {}", repo.display());
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
    fn auto_review_uses_containing_repository_from_nested_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("src").join("nested");
        init_repo(&repo);
        std::fs::create_dir_all(&nested).expect("create nested cwd");

        let target = resolve_auto_review_target(&scope(
            nested,
            vec![repo.canonicalize().expect("canonical repository")],
        ))
        .expect("valid repository scope");

        assert_eq!(
            target,
            AutoReviewTarget::IsolatedRepository(
                repo.canonicalize().expect("canonical repository")
            )
        );
    }

    #[test]
    fn auto_review_uses_workspace_mode_for_multiple_nested_repositories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let repo_b = workspace.join("b-repo");
        let repo_a = workspace.join("a-repo");
        init_repo(&repo_b);
        init_repo(&repo_a);

        let target = resolve_auto_review_target(&scope(
            workspace,
            vec![
                repo_a.canonicalize().expect("canonical first repository"),
                repo_b.canonicalize().expect("canonical second repository"),
            ],
        ))
        .expect("valid multi-repository scope");

        assert_eq!(
            target,
            AutoReviewTarget::Workspace {
                repository_roots: vec![
                    repo_a.canonicalize().expect("canonical first repository"),
                    repo_b.canonicalize().expect("canonical second repository"),
                ],
            }
        );
    }

    #[test]
    fn auto_review_rejects_scope_when_no_repository_exists() {
        let temp = tempfile::tempdir().expect("tempdir");

        let error = resolve_auto_review_target(&scope(temp.path().to_path_buf(), Vec::new()))
            .expect_err("an unresolved scope must not review a broad working directory");

        assert!(error.contains("No Git repositories"));
    }

    #[test]
    fn auto_review_rejects_malformed_repository_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join(".git"), "not a gitdir declaration\n")
            .expect("write malformed git marker");

        let error = resolve_auto_review_target(&scope(temp.path().to_path_buf(), Vec::new()))
            .expect_err("malformed repository state must not widen review scope");

        assert!(error.contains("No Git repositories"));
    }
}
