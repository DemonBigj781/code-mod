//! Persistent project scopes used by snapshots, reviews, and direct slash aliases.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FILENAME: &str = "projects.json";
const LOCK_FILENAME: &str = "projects.lock";
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const MAX_GIT_SEARCH_DEPTH: u16 = 64;

pub const HELP: &str = "/project [list|show <name>|use <name>|<name>|add <master|group|single> <dir> <name> <git_search_depth>|remove <name>|remove <master|group|single> <dir> <name> <git_search_depth>]";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    Master,
    Group,
    Single,
}

impl ProjectKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "master" => Ok(Self::Master),
            "group" => Ok(Self::Group),
            "single" => Ok(Self::Single),
            _ => bail!("project kind must be master, group, or single"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Group => "group",
            Self::Single => "single",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectDefinition {
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectKind,
    pub git_search_depth: u16,
}

impl ProjectDefinition {
    fn validate_structure(&self) -> Result<()> {
        project_name_key(&self.name)?;
        ensure!(
            self.path.is_absolute(),
            "project directory must be absolute"
        );
        validate_depth(self.kind, self.git_search_depth)
    }

    pub fn validate_path(&self) -> Result<()> {
        ensure!(
            self.path.exists(),
            "project directory does not exist: {}",
            self.path.display()
        );
        ensure!(
            self.path.is_dir(),
            "project path is not a directory: {}",
            self.path.display()
        );
        Ok(())
    }

    pub fn alias(&self) -> String {
        project_name_key(&self.name).expect("validated project name")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectCommand {
    List,
    Show(String),
    Use(String),
    Add {
        kind: ProjectKind,
        directory: PathBuf,
        name: String,
        git_search_depth: u16,
    },
    Remove(String),
    RemoveExact {
        kind: ProjectKind,
        directory: PathBuf,
        name: String,
        git_search_depth: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectCommandResult {
    pub lines: Vec<String>,
    pub switch_cwd: Option<PathBuf>,
    pub scope_changed: bool,
    pub registry_changed: bool,
}

impl ProjectCommand {
    pub fn parse(input: &str) -> Result<Self> {
        let args = shlex::split(input).context("invalid quoting in /project command")?;
        Self::parse_args(&args)
    }

    pub fn parse_args(args: &[String]) -> Result<Self> {
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        match args.as_slice() {
            [] | ["list"] => Ok(Self::List),
            ["show", name] => Ok(Self::Show((*name).to_owned())),
            ["use", name] => Ok(Self::Use((*name).to_owned())),
            ["add", kind, directory, name, depth] => Ok(Self::Add {
                kind: ProjectKind::parse(kind)?,
                directory: PathBuf::from(directory),
                name: (*name).to_owned(),
                git_search_depth: parse_depth(depth)?,
            }),
            ["remove", name] => Ok(Self::Remove((*name).to_owned())),
            ["remove", kind, directory, name, depth] => Ok(Self::RemoveExact {
                kind: ProjectKind::parse(kind)?,
                directory: PathBuf::from(directory),
                name: (*name).to_owned(),
                git_search_depth: parse_depth(depth)?,
            }),
            [name] => Ok(Self::Use((*name).to_owned())),
            _ => bail!(HELP),
        }
    }
}

pub fn resolve_project_directory(cwd: &Path, directory: &Path) -> PathBuf {
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

pub fn execute_project_command<I, S>(
    registry: &Registry,
    cwd: &Path,
    command: ProjectCommand,
    reserved_aliases: I,
) -> Result<ProjectCommandResult>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    match command {
        ProjectCommand::List => {
            let projects = registry.list()?;
            let active = registry.active()?.map(|project| project.alias());
            let mut lines = vec![format!("Project registry: {} project(s).", projects.len())];
            if projects.is_empty() {
                lines.push(
                    "No project is registered; broad startup directories are not scanned recursively."
                        .to_owned(),
                );
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
            Ok(ProjectCommandResult {
                lines,
                switch_cwd: None,
                scope_changed: false,
                registry_changed: false,
            })
        }
        ProjectCommand::Show(name) => {
            let project = registry.get(&name)?;
            Ok(ProjectCommandResult {
                lines: vec![format!(
                    "/{} [{} depth={}] {}",
                    project.name,
                    project.kind.label(),
                    project.git_search_depth,
                    project.path.display()
                )],
                switch_cwd: None,
                scope_changed: false,
                registry_changed: false,
            })
        }
        ProjectCommand::Use(name) => {
            let (project, scope_changed) = registry.activate_with_change(&name)?;
            Ok(ProjectCommandResult {
                lines: vec![format!(
                    "Selected {} project `{}` at {} (Git search depth {}).",
                    project.kind.label(),
                    project.name,
                    project.path.display(),
                    project.git_search_depth
                )],
                switch_cwd: Some(project.path),
                scope_changed,
                registry_changed: scope_changed,
            })
        }
        ProjectCommand::Add {
            kind,
            directory,
            name,
            git_search_depth,
        } => {
            let directory = resolve_project_directory(cwd, &directory);
            let active_before = registry.active()?.map(|project| project.alias());
            let project =
                registry.add(kind, &directory, &name, git_search_depth, reserved_aliases)?;
            let active_after = registry.active()?;
            let scope_changed =
                active_before != active_after.as_ref().map(ProjectDefinition::alias);
            Ok(ProjectCommandResult {
                lines: vec![format!(
                    "Registered /{} as a {} project at {} with Git search depth {}.",
                    project.name,
                    project.kind.label(),
                    project.path.display(),
                    project.git_search_depth
                )],
                switch_cwd: scope_changed
                    .then(|| active_after.map(|active| active.path))
                    .flatten(),
                scope_changed,
                registry_changed: true,
            })
        }
        ProjectCommand::Remove(name) => remove_project(registry, &name, || registry.remove(&name)),
        ProjectCommand::RemoveExact {
            kind,
            directory,
            name,
            git_search_depth,
        } => {
            let directory = resolve_project_directory(cwd, &directory);
            remove_project(registry, &name, || {
                registry.remove_exact(kind, &directory, &name, git_search_depth)
            })
        }
    }
}

fn remove_project(
    registry: &Registry,
    name: &str,
    remove: impl FnOnce() -> Result<RegistrySelection>,
) -> Result<ProjectCommandResult> {
    let active_before = registry.active()?.map(|project| project.alias());
    let selection = remove()?;
    let scope_changed = active_before != selection.active;
    let switch_cwd = if scope_changed {
        selection
            .active
            .as_deref()
            .map(|active| registry.get(active).map(|project| project.path))
            .transpose()?
    } else {
        None
    };
    Ok(ProjectCommandResult {
        lines: vec![format!("Removed project `{name}`.")],
        switch_cwd,
        scope_changed,
        registry_changed: true,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrySelection {
    pub active: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    version: u8,
    active: Option<String>,
    projects: BTreeMap<String, ProjectDefinition>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            version: 1,
            active: None,
            projects: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct Registry {
    home: PathBuf,
}

impl Registry {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }

    pub fn list(&self) -> Result<Vec<ProjectDefinition>> {
        Ok(self.load()?.projects.into_values().collect())
    }

    pub fn active(&self) -> Result<Option<ProjectDefinition>> {
        let registry = self.load()?;
        Ok(registry
            .active
            .as_ref()
            .and_then(|key| registry.projects.get(key))
            .cloned())
    }

    pub fn get(&self, name: &str) -> Result<ProjectDefinition> {
        self.load()?
            .projects
            .remove(&project_name_key(name)?)
            .context("project is not registered")
    }

    pub fn resolve_alias(&self, name: &str) -> Result<Option<ProjectDefinition>> {
        let key = match project_name_key(name) {
            Ok(key) => key,
            Err(_) => return Ok(None),
        };
        Ok(self.load()?.projects.get(&key).cloned())
    }

    pub fn add<I, S>(
        &self,
        kind: ProjectKind,
        directory: &Path,
        name: &str,
        git_search_depth: u16,
        reserved_aliases: I,
    ) -> Result<ProjectDefinition>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let key = validate_project_alias(name, reserved_aliases)?;
        validate_depth(kind, git_search_depth)?;
        let path = canonical_project_directory(directory)?;
        let (_lock, mut registry) = self.locked()?;
        ensure!(
            !registry.projects.contains_key(&key),
            "project name already exists"
        );
        ensure!(
            !registry
                .projects
                .values()
                .any(|project| project.path == path),
            "project directory already exists"
        );
        if kind == ProjectKind::Master {
            ensure!(
                !registry
                    .projects
                    .values()
                    .any(|project| project.kind == ProjectKind::Master),
                "a master project is already registered"
            );
        }
        let project = ProjectDefinition {
            name: name.to_owned(),
            path,
            kind,
            git_search_depth,
        };
        project.validate_structure()?;
        let should_activate = match registry
            .active
            .as_ref()
            .and_then(|active| registry.projects.get(active))
        {
            None => true,
            Some(active) => active.kind == ProjectKind::Master && kind != ProjectKind::Master,
        };
        registry.projects.insert(key.clone(), project.clone());
        if should_activate {
            registry.active = Some(key);
        }
        self.write(&registry)?;
        Ok(project)
    }

    pub fn activate(&self, name: &str) -> Result<ProjectDefinition> {
        self.activate_with_change(name).map(|(project, _)| project)
    }

    fn activate_with_change(&self, name: &str) -> Result<(ProjectDefinition, bool)> {
        let key = project_name_key(name)?;
        let (_lock, mut registry) = self.locked()?;
        let project = registry
            .projects
            .get(&key)
            .cloned()
            .context("project is not registered")?;
        project.validate_path()?;
        let changed = registry.active.as_deref() != Some(key.as_str());
        if changed {
            registry.active = Some(key);
            self.write(&registry)?;
        }
        Ok((project, changed))
    }

    pub fn remove(&self, name: &str) -> Result<RegistrySelection> {
        self.remove_inner(name, None)
    }

    pub fn remove_exact(
        &self,
        kind: ProjectKind,
        directory: &Path,
        name: &str,
        git_search_depth: u16,
    ) -> Result<RegistrySelection> {
        validate_depth(kind, git_search_depth)?;
        let expected_path = canonical_project_directory(directory)?;
        let expected = ProjectDefinition {
            name: name.to_owned(),
            path: expected_path,
            kind,
            git_search_depth,
        };
        self.remove_inner(name, Some(&expected))
    }

    fn remove_inner(
        &self,
        name: &str,
        expected: Option<&ProjectDefinition>,
    ) -> Result<RegistrySelection> {
        let key = project_name_key(name)?;
        let (_lock, mut registry) = self.locked()?;
        let current = registry
            .projects
            .get(&key)
            .context("project is not registered")?;
        if let Some(expected) = expected {
            ensure!(
                current == expected,
                "registered project does not match the supplied kind, directory, name, and depth"
            );
        }
        registry.projects.remove(&key);
        if registry.active.as_deref() == Some(key.as_str()) {
            registry.active = registry
                .projects
                .iter()
                .find_map(|(candidate_key, project)| {
                    (project.kind == ProjectKind::Master).then(|| candidate_key.clone())
                });
        }
        let active = registry.active.clone();
        self.write(&registry)?;
        Ok(RegistrySelection { active })
    }

    fn load(&self) -> Result<RegistryFile> {
        let path = self.home.join(FILENAME);
        ensure!(!path.is_symlink(), "project registry must not be a symlink");
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RegistryFile::default());
            }
            Err(error) => return Err(error).context("cannot read project registry"),
        };
        ensure!(
            file.metadata()?.is_file(),
            "project registry must be a regular file"
        );
        let mut bytes = Vec::new();
        file.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_REGISTRY_BYTES,
            "project registry exceeds its 1 MiB storage budget"
        );
        let mut registry: RegistryFile = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("project registry is malformed; it has not been replaced")
        })?;
        if registry.active.as_deref() == Some("") {
            registry.active = None;
        }
        self.validate_registry(&registry)?;
        Ok(registry)
    }

    fn validate_registry(&self, registry: &RegistryFile) -> Result<()> {
        ensure!(
            registry.version == 1,
            "unsupported project registry version"
        );
        let mut paths = HashSet::new();
        let mut master_count = 0usize;
        for (key, project) in &registry.projects {
            project.validate_structure()?;
            ensure!(project.alias() == *key, "project registry name mismatch");
            ensure!(
                paths.insert(project.path.clone()),
                "project registry contains duplicate directories"
            );
            if project.kind == ProjectKind::Master {
                master_count += 1;
            }
        }
        ensure!(
            master_count <= 1,
            "project registry contains multiple master entries"
        );
        if let Some(active) = &registry.active {
            ensure!(
                registry.projects.contains_key(active),
                "active project is not registered"
            );
        }
        Ok(())
    }

    fn locked(&self) -> Result<(File, RegistryFile)> {
        std::fs::create_dir_all(&self.home)?;
        let path = self.home.join(LOCK_FILENAME);
        ensure!(
            !path.is_symlink(),
            "project registry lock must not be a symlink"
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(path)?;
        let start = Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error).context("project registry is busy or cannot be locked");
                }
            }
        }
        Ok((lock, self.load()?))
    }

    fn write(&self, registry: &RegistryFile) -> Result<()> {
        self.validate_registry(registry)?;
        let mut bytes = serde_json::to_vec_pretty(registry)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() as u64 <= MAX_REGISTRY_BYTES,
            "project registry exceeds its 1 MiB storage budget"
        );
        let mut file = tempfile::NamedTempFile::new_in(&self.home)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.home.join(FILENAME))
            .context("cannot save project registry")?;
        Ok(())
    }
}

pub fn validate_project_alias<I, S>(name: &str, reserved_aliases: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let key = project_name_key(name)?;
    ensure!(
        !reserved_aliases
            .into_iter()
            .any(|reserved| reserved.as_ref().eq_ignore_ascii_case(&key)),
        "project name conflicts with a reserved slash command"
    );
    Ok(key)
}

fn project_name_key(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty()
            && name == name.trim()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "project name must be 1-64 ASCII letters, numbers, dots, underscores, or dashes"
    );
    Ok(name.to_ascii_lowercase())
}

fn parse_depth(value: &str) -> Result<u16> {
    let depth = value
        .parse::<u16>()
        .context("git_search_depth must be a non-negative integer")?;
    ensure!(
        depth <= MAX_GIT_SEARCH_DEPTH,
        "git_search_depth must be at most {MAX_GIT_SEARCH_DEPTH}"
    );
    Ok(depth)
}

fn validate_depth(kind: ProjectKind, depth: u16) -> Result<()> {
    ensure!(
        depth <= MAX_GIT_SEARCH_DEPTH,
        "git_search_depth must be at most {MAX_GIT_SEARCH_DEPTH}"
    );
    if kind == ProjectKind::Single {
        ensure!(depth == 0, "single projects require git_search_depth 0");
    }
    Ok(())
}

fn canonical_project_directory(directory: &Path) -> Result<PathBuf> {
    ensure!(
        directory.exists(),
        "project directory does not exist: {}",
        directory.display()
    );
    ensure!(
        directory.is_dir(),
        "project path is not a directory: {}",
        directory.display()
    );
    directory
        .canonicalize()
        .with_context(|| format!("cannot resolve project directory: {}", directory.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parser_accepts_requested_full_forms_and_selection_aliases() {
        assert_eq!(
            ProjectCommand::parse("add group '/tmp/my projects' games 3").unwrap(),
            ProjectCommand::Add {
                kind: ProjectKind::Group,
                directory: PathBuf::from("/tmp/my projects"),
                name: "games".to_owned(),
                git_search_depth: 3,
            }
        );
        assert_eq!(
            ProjectCommand::parse("remove group '/tmp/my projects' games 3").unwrap(),
            ProjectCommand::RemoveExact {
                kind: ProjectKind::Group,
                directory: PathBuf::from("/tmp/my projects"),
                name: "games".to_owned(),
                git_search_depth: 3,
            }
        );
        assert_eq!(
            ProjectCommand::parse("code").unwrap(),
            ProjectCommand::Use("code".to_owned())
        );
        assert_eq!(
            ProjectCommand::parse_args(&["show".to_owned(), "code".to_owned()]).unwrap(),
            ProjectCommand::Show("code".to_owned())
        );
    }

    #[test]
    fn shared_command_execution_reports_scope_changes_for_all_entry_points() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let project = cwd.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let registry = Registry::new(home.path());

        let added = execute_project_command(
            &registry,
            cwd.path(),
            ProjectCommand::Add {
                kind: ProjectKind::Single,
                directory: PathBuf::from("project"),
                name: "demo".to_owned(),
                git_search_depth: 0,
            },
            ["help"],
        )
        .unwrap();
        assert!(added.registry_changed);
        assert!(added.scope_changed);
        assert_eq!(added.switch_cwd, Some(project.canonicalize().unwrap()));

        let selected = execute_project_command(
            &registry,
            cwd.path(),
            ProjectCommand::Use("demo".to_owned()),
            std::iter::empty::<&str>(),
        )
        .unwrap();
        assert!(!selected.scope_changed);
        assert!(!selected.registry_changed);
        assert_eq!(selected.switch_cwd, added.switch_cwd);

        let removed = execute_project_command(
            &registry,
            cwd.path(),
            ProjectCommand::Remove("demo".to_owned()),
            std::iter::empty::<&str>(),
        )
        .unwrap();
        assert!(removed.scope_changed);
        assert_eq!(removed.switch_cwd, None);
        assert!(registry.list().unwrap().is_empty());
    }

    #[test]
    fn registry_persists_projects_and_active_selection_across_instances() {
        let home = tempfile::tempdir().unwrap();
        let master = tempfile::tempdir().unwrap();
        let selected = tempfile::tempdir().unwrap();

        let registry = Registry::new(home.path());
        registry
            .add(
                ProjectKind::Master,
                master.path(),
                "all-projects",
                2,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        registry
            .add(
                ProjectKind::Single,
                selected.path(),
                "code",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        registry.activate("code").unwrap();

        let reloaded = Registry::new(home.path());
        assert_eq!(reloaded.list().unwrap().len(), 2);
        assert_eq!(reloaded.active().unwrap().unwrap().alias(), "code");
        assert_eq!(
            reloaded.resolve_alias("CoDe").unwrap().unwrap().alias(),
            "code"
        );
    }

    #[test]
    fn registry_rejects_duplicate_names_paths_and_reserved_aliases() {
        let home = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let registry = Registry::new(home.path());

        registry
            .add(
                ProjectKind::Single,
                first.path(),
                "code",
                0,
                ["help", "review"],
            )
            .unwrap();
        assert!(
            registry
                .add(
                    ProjectKind::Single,
                    second.path(),
                    "CODE",
                    0,
                    std::iter::empty::<&str>(),
                )
                .unwrap_err()
                .to_string()
                .contains("name already exists")
        );
        assert!(
            registry
                .add(
                    ProjectKind::Single,
                    first.path(),
                    "other",
                    0,
                    std::iter::empty::<&str>(),
                )
                .unwrap_err()
                .to_string()
                .contains("directory already exists")
        );
        assert!(
            registry
                .add(ProjectKind::Single, second.path(), "help", 0, ["help"])
                .unwrap_err()
                .to_string()
                .contains("reserved slash command")
        );
    }

    #[test]
    fn removing_the_active_project_falls_back_to_master() {
        let home = tempfile::tempdir().unwrap();
        let master = tempfile::tempdir().unwrap();
        let single = tempfile::tempdir().unwrap();
        let registry = Registry::new(home.path());

        registry
            .add(
                ProjectKind::Master,
                master.path(),
                "all-projects",
                2,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        registry
            .add(
                ProjectKind::Single,
                single.path(),
                "code",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        registry.activate("code").unwrap();

        let selection = registry.remove("code").unwrap();

        assert_eq!(selection.active.as_deref(), Some("all-projects"));
        assert_eq!(registry.active().unwrap().unwrap().name, "all-projects");
    }

    #[test]
    fn master_is_a_fallback_and_never_steals_a_narrower_active_scope() {
        let home = tempfile::tempdir().unwrap();
        let master = tempfile::tempdir().unwrap();
        let single = tempfile::tempdir().unwrap();
        let registry = Registry::new(home.path());

        registry
            .add(
                ProjectKind::Master,
                master.path(),
                "master",
                1,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        assert_eq!(registry.active().unwrap().unwrap().name, "master");

        registry
            .add(
                ProjectKind::Single,
                single.path(),
                "yata",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        assert_eq!(registry.active().unwrap().unwrap().name, "yata");

        let second_home = tempfile::tempdir().unwrap();
        let second_master = tempfile::tempdir().unwrap();
        let second_single = tempfile::tempdir().unwrap();
        let second = Registry::new(second_home.path());
        second
            .add(
                ProjectKind::Single,
                second_single.path(),
                "yata",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        second
            .add(
                ProjectKind::Master,
                second_master.path(),
                "master",
                1,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        assert_eq!(second.active().unwrap().unwrap().name, "yata");
    }

    #[test]
    fn deleted_project_path_is_reported_without_destroying_the_registry() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let path = project.path().to_path_buf();
        let registry = Registry::new(home.path());
        registry
            .add(
                ProjectKind::Single,
                &path,
                "temporary",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        drop(project);

        let active = registry.active().unwrap().unwrap();
        assert_eq!(active.path, path.canonicalize().unwrap_or(path));
        assert!(
            active
                .validate_path()
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn malformed_registry_is_reported_without_replacing_the_file() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(FILENAME);
        std::fs::write(&path, b"{not valid json\n").unwrap();
        let original = std::fs::read(&path).unwrap();

        let error = Registry::new(home.path()).list().unwrap_err().to_string();

        assert!(error.contains("malformed"));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn empty_legacy_active_value_is_treated_as_no_selection() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(FILENAME);
        std::fs::write(
            &path,
            br#"{
  "version": 1,
  "active": "",
  "projects": {}
}
"#,
        )
        .unwrap();
        let original = std::fs::read(&path).unwrap();

        let registry = Registry::new(home.path());

        assert!(registry.active().unwrap().is_none());
        assert!(registry.list().unwrap().is_empty());
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
