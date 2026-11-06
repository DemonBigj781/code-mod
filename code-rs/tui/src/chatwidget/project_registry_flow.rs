use super::*;
use code_core::project_registry::{HELP, ProjectCommand, ProjectDefinition, ProjectKind, Registry};

fn scope_error(key: String, label: String, root: PathBuf, error: impl ToString) -> RepositoryScope {
    RepositoryScope {
        key,
        label,
        root,
        repositories: Vec::new(),
        error: Some(error.to_string()),
    }
}

fn project_scope_key(project: &ProjectDefinition) -> String {
    format!(
        "project:{}:{}:{}:{}",
        project.alias(),
        project.kind.label(),
        project.path.display(),
        project.git_search_depth
    )
}

fn project_alias_conflicts_with_builtin(name: &str) -> bool {
    name.to_ascii_lowercase().parse::<SlashCommand>().is_ok()
        || name.eq_ignore_ascii_case("exit")
}

fn is_branch_worktree_override(cwd: &Path, project: &ProjectDefinition) -> bool {
    cwd != project.path
        && cwd
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/.code/branches/")
}

fn resolve_repository_scope(code_home: &Path, cwd: &Path) -> RepositoryScope {
    let registry = Registry::new(code_home);
    match registry.active() {
        Ok(Some(project)) => {
            if is_branch_worktree_override(cwd, &project) {
                return match discover_repository_roots_with_depth(cwd, 0) {
                    Ok(repositories) => RepositoryScope {
                        key: format!("worktree:{}", cwd.display()),
                        label: format!("branch worktree {}", cwd.display()),
                        root: cwd.to_path_buf(),
                        repositories,
                        error: None,
                    },
                    Err(error) => scope_error(
                        format!("worktree:{}", cwd.display()),
                        format!("branch worktree {}", cwd.display()),
                        cwd.to_path_buf(),
                        error,
                    ),
                };
            }
            let key = project_scope_key(&project);
            let label = format!("{} project `{}`", project.kind.label(), project.name);
            if let Err(error) = project.validate_path() {
                return scope_error(key, label, project.path, error);
            }
            let result = match project.kind {
                ProjectKind::Single => discover_repository_roots_with_depth(&project.path, 0),
                ProjectKind::Master | ProjectKind::Group => discover_repository_roots_with_depth(
                    &project.path,
                    usize::from(project.git_search_depth),
                ),
            };
            match result {
                Ok(repositories) => RepositoryScope {
                    key,
                    label,
                    root: project.path,
                    repositories,
                    error: None,
                },
                Err(error) => scope_error(key, label, project.path, error),
            }
        }
        Ok(None) => match discover_repository_roots_with_depth(cwd, 0) {
            Ok(repositories) => RepositoryScope {
                key: format!("cwd:{}", cwd.display()),
                label: format!("working directory fallback `{}`", cwd.display()),
                root: cwd.to_path_buf(),
                repositories,
                error: None,
            },
            Err(_) => scope_error(
                format!("unregistered:{}", cwd.display()),
                "unregistered working directory".to_owned(),
                cwd.to_path_buf(),
                format!(
                    "No project is selected and {} is not inside a Git repository. Register the intended scope with /project add <master|group|single> <dir> <name> <git_search_depth>.",
                    cwd.display()
                ),
            ),
        },
        Err(error) => scope_error(
            format!("registry-error:{}", cwd.display()),
            "project registry".to_owned(),
            cwd.to_path_buf(),
            error,
        ),
    }
}

fn project_directory_from_command(cwd: &Path, directory: &Path) -> PathBuf {
    if directory.is_absolute() {
        return directory.to_path_buf();
    }
    if let Some(text) = directory.to_str()
        && let Some(suffix) = text.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(suffix);
    }
    cwd.join(directory)
}

fn clear_in_flight_snapshot_state<T>(
    queue: &mut VecDeque<(u64, T)>,
    active: &mut Option<(u64, T)>,
) {
    queue.clear();
    *active = None;
}

impl ChatWidget<'_> {
    pub(in crate::chatwidget) fn repository_scope(&self) -> RepositoryScope {
        resolve_repository_scope(&self.config.code_home, &self.config.cwd)
    }

    fn project_reserved_aliases(&self) -> Vec<String> {
        let mut aliases = crate::slash_command::built_in_slash_commands()
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        aliases.extend(
            self.config
                .subagent_commands
                .iter()
                .map(|command| command.name.clone()),
        );
        aliases.extend(
            self.config
                .project_commands
                .iter()
                .map(|command| command.name.clone()),
        );
        aliases.extend(
            self.bottom_pane
                .custom_prompts()
                .iter()
                .map(|prompt| prompt.name.clone()),
        );
        aliases
    }

    pub(in crate::chatwidget) fn invalidate_ghost_snapshots_for_scope_change(&mut self) {
        self.ghost_snapshots.clear();
        clear_in_flight_snapshot_state(
            &mut self.ghost_snapshot_queue,
            &mut self.active_ghost_snapshot,
        );
        self.ghost_snapshots_disabled = false;
        self.ghost_snapshots_disabled_reason = None;
    }

    fn refresh_project_aliases(&mut self) {
        let names = Registry::new(&self.config.code_home)
            .list()
            .map(|projects| projects.into_iter().map(|project| project.name).collect())
            .unwrap_or_default();
        self.bottom_pane.set_project_aliases(names);
    }

    pub(in crate::chatwidget) fn handle_project_alias(&mut self, name: &str) -> bool {
        let registry = Registry::new(&self.config.code_home);
        let project = match registry.resolve_alias(name) {
            Ok(Some(project)) => project,
            Ok(None) => return false,
            Err(error) => {
                self.debug_notice(format!("Project registry: {error}"));
                return true;
            }
        };
        self.invalidate_ghost_snapshots_for_scope_change();
        match registry.activate(&project.name) {
            Ok(project) => {
                self.debug_notice(format!(
                    "Selected {} project `{}` at {} (Git search depth {}).",
                    project.kind.label(),
                    project.name,
                    project.path.display(),
                    project.git_search_depth
                ));
                self.app_event_tx
                    .send(AppEvent::SwitchCwd(project.path, None));
            }
            Err(error) => self.debug_notice(format!("Project registry: {error}")),
        }
        true
    }

    pub(crate) fn handle_project_registry_command(&mut self, args: String) {
        if matches!(args.trim(), "help" | "?") {
            self.debug_notice(HELP.to_owned());
            return;
        }
        let command = match ProjectCommand::parse(&args) {
            Ok(command) => command,
            Err(error) => {
                self.debug_notice(format!("Project registry: {error}"));
                return;
            }
        };
        let registry = Registry::new(&self.config.code_home);
        let result = match command {
            ProjectCommand::List => registry.list().and_then(|projects| {
                let active = registry.active()?.map(|project| project.alias());
                let mut lines = vec![format!(
                    "Project registry: {} project(s).",
                    projects.len()
                )];
                if projects.is_empty() {
                    lines.push("No project is registered; broad startup directories are not scanned recursively.".to_owned());
                }
                lines.extend(projects.into_iter().map(|project| {
                    let marker = if active.as_deref() == Some(project.alias().as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    format!(
                        "{marker} /{} [{} depth={}] {}",
                        project.name,
                        project.kind.label(),
                        project.git_search_depth,
                        project.path.display()
                    )
                }));
                lines.push(HELP.to_owned());
                Ok(lines)
            }),
            ProjectCommand::Show(name) => registry.get(&name).map(|project| {
                vec![format!(
                    "/{} [{} depth={}] {}",
                    project.name,
                    project.kind.label(),
                    project.git_search_depth,
                    project.path.display()
                )]
            }),
            ProjectCommand::Use(name) => {
                self.handle_project_alias(&name);
                return;
            }
            ProjectCommand::Add {
                kind,
                directory,
                name,
                git_search_depth,
            } => {
                let directory = project_directory_from_command(&self.config.cwd, &directory);
                let active_before = registry.active().ok().flatten().map(|project| project.alias());
                let mut reserved_aliases = self.project_reserved_aliases();
                if project_alias_conflicts_with_builtin(&name) {
                    reserved_aliases.push(name.clone());
                }
                registry
                    .add(
                        kind,
                        &directory,
                        &name,
                        git_search_depth,
                        reserved_aliases,
                    )
                    .map(|project| {
                        let active_after = registry.active().ok().flatten().map(|entry| entry.alias());
                        if active_before != active_after {
                            self.invalidate_ghost_snapshots_for_scope_change();
                            self.app_event_tx
                                .send(AppEvent::SwitchCwd(project.path.clone(), None));
                        }
                        vec![format!(
                            "Registered /{} as a {} project at {} with Git search depth {}.",
                            project.name,
                            project.kind.label(),
                            project.path.display(),
                            project.git_search_depth
                        )]
                    })
            }
            ProjectCommand::Remove(name) => {
                let active_before = registry.active().ok().flatten().map(|project| project.alias());
                registry.remove(&name).and_then(|selection| {
                    let active_after = selection.active.clone();
                    if active_before != active_after {
                        self.invalidate_ghost_snapshots_for_scope_change();
                        if let Some(active) = active_after.as_deref() {
                            let fallback = registry.get(active)?;
                            self.app_event_tx
                                .send(AppEvent::SwitchCwd(fallback.path, None));
                        }
                    }
                    Ok(vec![format!("Removed project `{name}`.")])
                })
            }
            ProjectCommand::RemoveExact {
                kind,
                directory,
                name,
                git_search_depth,
            } => {
                let directory = project_directory_from_command(&self.config.cwd, &directory);
                let active_before = registry.active().ok().flatten().map(|project| project.alias());
                registry
                    .remove_exact(kind, &directory, &name, git_search_depth)
                    .and_then(|selection| {
                        let active_after = selection.active.clone();
                        if active_before != active_after {
                            self.invalidate_ghost_snapshots_for_scope_change();
                            if let Some(active) = active_after.as_deref() {
                                let fallback = registry.get(active)?;
                                self.app_event_tx
                                    .send(AppEvent::SwitchCwd(fallback.path, None));
                            }
                        }
                        Ok(vec![format!("Removed project `{name}`.")])
                    })
            }
        };
        self.refresh_project_aliases();
        match result {
            Ok(lines) => self.history_push_plain_paragraphs(PlainMessageKind::Notice, lines),
            Err(error) => self.debug_notice(format!("Project registry: {error}")),
        }
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_core::project_registry::{ProjectKind, Registry};
    use std::process::Command;

    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        let output = Command::new("git")
            .current_dir(path)
            .args(["init", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn declared_single_project_excludes_unrelated_repositories_under_startup_cwd() {
        let root = tempfile::tempdir().unwrap();
        let code_home = tempfile::tempdir().unwrap();
        let selected = root.path().join("projects/code");
        let unrelated = root.path().join("other/repository");
        init_repo(&selected);
        init_repo(&unrelated);
        let registry = Registry::new(code_home.path());
        registry
            .add(
                ProjectKind::Single,
                &selected,
                "code",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();

        let scope = resolve_repository_scope(code_home.path(), root.path());

        assert!(scope.error.is_none(), "{:?}", scope.error);
        assert_eq!(scope.repositories, vec![selected.canonicalize().unwrap()]);
        assert!(
            !scope
                .repositories
                .contains(&unrelated.canonicalize().unwrap())
        );
    }

    #[test]
    fn declared_group_respects_git_search_depth() {
        let root = tempfile::tempdir().unwrap();
        let code_home = tempfile::tempdir().unwrap();
        let group = root.path().join("projects");
        let shallow = group.join("shallow");
        let second_shallow = group.join("second-shallow");
        let deep = group.join("one/two/deep");
        init_repo(&shallow);
        init_repo(&second_shallow);
        init_repo(&deep);
        let registry = Registry::new(code_home.path());
        registry
            .add(
                ProjectKind::Group,
                &group,
                "projects",
                1,
                std::iter::empty::<&str>(),
            )
            .unwrap();

        let scope = resolve_repository_scope(code_home.path(), root.path());

        assert_eq!(
            scope.repositories,
            vec![
                second_shallow.canonicalize().unwrap(),
                shallow.canonicalize().unwrap(),
            ]
        );
    }

    #[test]
    fn unregistered_broad_cwd_does_not_recursively_discover_children() {
        let root = tempfile::tempdir().unwrap();
        let code_home = tempfile::tempdir().unwrap();
        init_repo(&root.path().join("nested/repository"));

        let scope = resolve_repository_scope(code_home.path(), root.path());

        assert!(scope.repositories.is_empty());
        assert!(scope.error.unwrap().contains("/project add"));
    }

    #[test]
    fn switching_scope_releases_active_and_queued_snapshot_jobs() {
        let mut queue = VecDeque::from([(2, "queued")]);
        let mut active = Some((1, "active"));

        clear_in_flight_snapshot_state(&mut queue, &mut active);

        assert!(queue.is_empty());
        assert!(active.is_none());
    }

    #[test]
    fn alternate_builtin_slash_names_are_reserved_for_project_aliases() {
        for name in [
            "upgrade",
            "Upgrade",
            "context",
            "compact-at",
            "app",
            "plugin",
            "account",
            "guide",
            "exit",
        ] {
            assert!(project_alias_conflicts_with_builtin(name), "{name}");
        }
        assert!(!project_alias_conflicts_with_builtin("code-project"));
    }
}
