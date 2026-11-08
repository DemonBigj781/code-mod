use super::*;
use code_core::project_registry::{
    HELP, ProjectCommand, ProjectCommandResult, ProjectDefinition, ProjectKind, Registry,
    execute_project_command,
};

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

    pub(in crate::chatwidget) fn project_reserved_aliases(&self) -> Vec<String> {
        let mut aliases = crate::project_reserved_aliases();
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

    pub(crate) fn apply_project_command_result(&mut self, result: ProjectCommandResult) {
        if result.scope_changed {
            self.invalidate_ghost_snapshots_for_scope_change();
        }
        if result.registry_changed {
            self.refresh_project_aliases();
        }
        if !result.lines.is_empty() {
            self.history_push_plain_paragraphs(PlainMessageKind::Notice, result.lines);
        }
        if let Some(path) = result.switch_cwd.filter(|path| path != &self.config.cwd) {
            self.app_event_tx.send(AppEvent::SwitchCwd(path, None));
        }
        self.request_redraw();
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
        match execute_project_command(
            &registry,
            &self.config.cwd,
            ProjectCommand::Use(project.name),
            std::iter::empty::<&str>(),
        ) {
            Ok(result) => self.apply_project_command_result(result),
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
        match execute_project_command(
            &registry,
            &self.config.cwd,
            command,
            self.project_reserved_aliases(),
        ) {
            Ok(result) => self.apply_project_command_result(result),
            Err(error) => self.debug_notice(format!("Project registry: {error}")),
        }
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
