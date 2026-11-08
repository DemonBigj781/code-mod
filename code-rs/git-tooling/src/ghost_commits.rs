use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use tempfile::Builder;
use walkdir::WalkDir;

use crate::GhostCommit;
use crate::GitToolingError;
use crate::operations::apply_repo_prefix_to_force_include;
use crate::operations::ensure_git_repository;
use crate::operations::ensure_git_repository_with_timeout;
use crate::operations::normalize_relative_path;
use crate::operations::repo_subdir;
use crate::operations::resolve_head_with_timeout;
use crate::operations::resolve_repository_root;
use crate::operations::resolve_repository_root_with_timeout;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_status_with_timeout;
use crate::operations::run_git_for_stdout_bytes_with_timeout;
use crate::operations::run_git_for_stdout_with_timeout;

/// Default commit message used for ghost commits when none is provided.
const DEFAULT_COMMIT_MESSAGE: &str = "code snapshot";

const REPOSITORY_DISCOVERY_SKIP_DIRS: &[&str] = &[
    ".cache",
    ".cargo",
    ".code",
    ".codex",
    ".local",
    ".npm",
    ".pnpm-store",
    ".rustup",
    ".steam",
    ".var",
    "__pycache__",
    "node_modules",
    "target",
    "venv",
    ".venv",
];

/// Find the Git repositories represented by a workspace path.
///
/// If the path is already inside a repository, only that repository is returned. Otherwise the
/// workspace is searched recursively for nested repositories. Symbolic links are not followed and
/// common generated/cache directories are pruned so a broad start directory remains bounded.
pub fn discover_repository_roots(workspace_path: &Path) -> Result<Vec<PathBuf>, GitToolingError> {
    discover_repository_roots_inner(workspace_path, None)
}

/// Find Git repository roots no more than `max_depth` directory levels below a workspace.
///
/// A depth of zero accepts only a repository containing `workspace_path`; it never scans child
/// or sibling directories. The depth counts repository-root directories below the workspace, not
/// the `.git` marker itself.
pub fn discover_repository_roots_with_depth(
    workspace_path: &Path,
    max_depth: usize,
) -> Result<Vec<PathBuf>, GitToolingError> {
    discover_repository_roots_inner(workspace_path, Some(max_depth))
}

fn discover_repository_roots_inner(
    workspace_path: &Path,
    max_depth: Option<usize>,
) -> Result<Vec<PathBuf>, GitToolingError> {
    match ensure_git_repository(workspace_path) {
        Ok(()) => {
            let root = resolve_repository_root(workspace_path)?;
            return Ok(vec![root.canonicalize().unwrap_or(root)]);
        }
        Err(GitToolingError::NotAGitRepository { .. }) => {}
        Err(err) => return Err(err),
    }

    let mut roots = BTreeSet::new();
    let mut walker = WalkDir::new(workspace_path).follow_links(false);
    if let Some(max_depth) = max_depth {
        walker = walker.max_depth(max_depth.saturating_add(1));
    }
    let mut entries = walker.into_iter();

    while let Some(entry) = entries.next() {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy();
        if entry.depth() > 0
            && entry.file_type().is_dir()
            && REPOSITORY_DISCOVERY_SKIP_DIRS.contains(&file_name.as_ref())
        {
            entries.skip_current_dir();
            continue;
        }

        if file_name != ".git" {
            continue;
        }
        if entry.file_type().is_dir() {
            entries.skip_current_dir();
        }
        let Some(candidate) = entry.path().parent() else {
            continue;
        };
        match ensure_git_repository(candidate) {
            Ok(()) => {
                let root = resolve_repository_root(candidate)?;
                roots.insert(root.canonicalize().unwrap_or(root));
            }
            Err(GitToolingError::NotAGitRepository { .. }) => {}
            Err(err) => return Err(err),
        }
    }

    if roots.is_empty() {
        return Err(GitToolingError::NotAGitRepository {
            path: workspace_path.to_path_buf(),
        });
    }

    Ok(roots.into_iter().collect())
}

/// Options to control ghost commit creation.
pub struct CreateGhostCommitOptions<'a> {
    pub repo_path: &'a Path,
    pub message: Option<&'a str>,
    pub force_include: Vec<PathBuf>,
    pub parent: Option<&'a str>,
    pub post_commit_hook: Option<&'a dyn Fn()>,
    pub command_timeout: Option<Duration>,
}

impl<'a> CreateGhostCommitOptions<'a> {
    /// Creates options scoped to the provided repository path.
    pub fn new(repo_path: &'a Path) -> Self {
        Self {
            repo_path,
            message: None,
            force_include: Vec::new(),
            parent: None,
            post_commit_hook: None,
            command_timeout: None,
        }
    }

    /// Sets a custom commit message for the ghost commit.
    #[must_use]
    pub fn message(mut self, message: &'a str) -> Self {
        self.message = Some(message);
        self
    }

    /// Overrides the parent commit for the ghost snapshot when provided.
    #[must_use]
    pub fn parent(mut self, parent: &'a str) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Registers a hook to run after the ghost commit is created.
    #[must_use]
    pub fn post_commit_hook(mut self, hook: &'a dyn Fn()) -> Self {
        self.post_commit_hook = Some(hook);
        self
    }

    /// Limits the total time spent by Git subprocesses while creating this snapshot.
    #[must_use]
    pub fn command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = Some(timeout);
        self
    }

    /// Supplies the entire force-include path list at once.
    #[must_use]
    pub fn force_include<I>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.force_include = paths.into_iter().collect();
        self
    }

    /// Adds a single path to the force-include list.
    #[must_use]
    pub fn push_force_include<P>(mut self, path: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.force_include.push(path.into());
        self
    }
}

fn create_index_tempdir(home_dir: Option<&Path>) -> std::io::Result<tempfile::TempDir> {
    if let Some(home_dir) = home_dir {
        let home_tmp = home_dir.join("tmp");
        if std::fs::create_dir_all(&home_tmp).is_ok()
            && let Ok(tempdir) = Builder::new()
                .prefix("code-git-index-")
                .tempdir_in(home_tmp)
        {
            return Ok(tempdir);
        }
    }

    Builder::new().prefix("code-git-index-").tempdir()
}

/// Create a ghost commit capturing the current state of the repository's working tree.
pub fn create_ghost_commit(
    options: &CreateGhostCommitOptions<'_>,
) -> Result<GhostCommit, GitToolingError> {
    let deadline = options
        .command_timeout
        .map(|timeout| Instant::now() + timeout);
    ensure_git_repository_with_timeout(options.repo_path, remaining_timeout(deadline)?)?;

    let repo_root =
        resolve_repository_root_with_timeout(options.repo_path, remaining_timeout(deadline)?)?;
    let repo_prefix = repo_subdir(repo_root.as_path(), options.repo_path);
    let parent_override = options.parent.map(ToString::to_string);
    let resolved_parent =
        resolve_head_with_timeout(repo_root.as_path(), remaining_timeout(deadline)?)?;
    let parent_ref = parent_override
        .as_deref()
        .or(resolved_parent.as_deref())
        .map(ToString::to_string);

    let normalized_force = options
        .force_include
        .iter()
        .map(|path| normalize_relative_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    let force_include =
        apply_repo_prefix_to_force_include(repo_prefix.as_deref(), &normalized_force);
    let home_dir = dirs::home_dir();
    let index_tempdir = create_index_tempdir(home_dir.as_deref())?;
    let index_path = index_tempdir.path().join("index");
    let base_env = vec![(
        OsString::from("GIT_INDEX_FILE"),
        OsString::from(index_path.as_os_str()),
    )];

    // Start from the parent tree and update only paths reported by Git status.
    // An empty temporary index followed by `git add --all` re-hashes every
    // tracked file, which makes snapshots fail their deadline in very large
    // repositories even when only one file changed.
    if let Some(parent) = parent_ref.as_deref() {
        run_git_for_status_with_timeout(
            repo_root.as_path(),
            vec![OsString::from("read-tree"), OsString::from(parent)],
            Some(base_env.as_slice()),
            remaining_timeout(deadline)?,
        )?;
    }
    let changed_paths = capture_changed_paths(
        repo_root.as_path(),
        repo_prefix.as_deref(),
        remaining_timeout(deadline)?,
    )?;
    add_paths_to_index(
        repo_root.as_path(),
        base_env.as_slice(),
        &changed_paths,
        deadline,
    )?;
    if !force_include.is_empty() {
        let mut args = Vec::with_capacity(force_include.len() + 2);
        args.push(OsString::from("add"));
        args.push(OsString::from("--force"));
        args.extend(
            force_include
                .iter()
                .map(|path| OsString::from(path.as_os_str())),
        );
        run_git_for_status_with_timeout(
            repo_root.as_path(),
            args,
            Some(base_env.as_slice()),
            remaining_timeout(deadline)?,
        )?;
    }

    let tree_id = run_git_for_stdout_with_timeout(
        repo_root.as_path(),
        vec![OsString::from("write-tree")],
        Some(base_env.as_slice()),
        remaining_timeout(deadline)?,
    )?;

    let mut commit_env = base_env;
    commit_env.extend(default_commit_identity());
    let message = options.message.unwrap_or(DEFAULT_COMMIT_MESSAGE);
    let commit_args = {
        let mut result = vec![OsString::from("commit-tree"), OsString::from(&tree_id)];
        if let Some(parent) = parent_ref.as_deref() {
            result.extend([OsString::from("-p"), OsString::from(parent)]);
        }
        result.extend([OsString::from("-m"), OsString::from(message)]);
        result
    };

    // Retrieve commit ID.
    let commit_id = run_git_for_stdout_with_timeout(
        repo_root.as_path(),
        commit_args,
        Some(commit_env.as_slice()),
        remaining_timeout(deadline)?,
    )?;

    if let Some(hook) = options.post_commit_hook {
        hook();
    }

    Ok(GhostCommit::new(commit_id, parent_ref))
}

fn capture_changed_paths(
    repo_root: &Path,
    repo_prefix: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<Vec<PathBuf>, GitToolingError> {
    let mut args = vec![
        OsString::from("status"),
        OsString::from("--porcelain=1"),
        OsString::from("-z"),
        OsString::from("--untracked-files=all"),
    ];
    if let Some(prefix) = repo_prefix {
        args.extend([OsString::from("--"), prefix.as_os_str().to_os_string()]);
    }
    let (command, output) = run_git_for_stdout_bytes_with_timeout(repo_root, args, None, timeout)?;
    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    let mut index = 0usize;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            return Err(GitToolingError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unexpected output from `{command}`"),
            )));
        }
        paths.insert(path_from_git_bytes(&record[3..], &command)?);
        if matches!(record[0], b'R' | b'C') || matches!(record[1], b'R' | b'C') {
            let source = records.get(index).copied().unwrap_or_default();
            index += 1;
            if source.is_empty() {
                return Err(GitToolingError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("missing rename source from `{command}`"),
                )));
            }
            paths.insert(path_from_git_bytes(source, &command)?);
        }
    }
    Ok(paths.into_iter().collect())
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8], _command: &str) -> Result<PathBuf, GitToolingError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8], command: &str) -> Result<PathBuf, GitToolingError> {
    String::from_utf8(bytes.to_vec())
        .map(PathBuf::from)
        .map_err(|source| GitToolingError::GitOutputUtf8 {
            command: command.to_owned(),
            source,
        })
}

fn add_paths_to_index(
    repo_root: &Path,
    env: &[(OsString, OsString)],
    paths: &[PathBuf],
    deadline: Option<Instant>,
) -> Result<(), GitToolingError> {
    for chunk in paths.chunks(64) {
        let mut args = vec![
            OsString::from("add"),
            OsString::from("--all"),
            OsString::from("--"),
        ];
        args.extend(chunk.iter().map(|path| path.as_os_str().to_os_string()));
        run_git_for_status_with_timeout(repo_root, args, Some(env), remaining_timeout(deadline)?)?;
    }
    Ok(())
}

fn remaining_timeout(deadline: Option<Instant>) -> Result<Option<Duration>, GitToolingError> {
    let Some(deadline) = deadline else {
        return Ok(None);
    };
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(Some)
        .ok_or_else(|| {
            GitToolingError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "ghost snapshot Git command deadline exceeded",
            ))
        })
}

/// Restore the working tree to match the provided ghost commit.
pub fn restore_ghost_commit(repo_path: &Path, commit: &GhostCommit) -> Result<(), GitToolingError> {
    restore_to_commit(repo_path, commit.id())
}

/// Restore the working tree to match the given commit ID.
pub fn restore_to_commit(repo_path: &Path, commit_id: &str) -> Result<(), GitToolingError> {
    ensure_git_repository(repo_path)?;

    let repo_root = resolve_repository_root(repo_path)?;
    let repo_prefix = repo_subdir(repo_root.as_path(), repo_path);

    let mut restore_args = vec![
        OsString::from("restore"),
        OsString::from("--source"),
        OsString::from(commit_id),
        OsString::from("--worktree"),
        OsString::from("--staged"),
        OsString::from("--"),
    ];
    if let Some(prefix) = repo_prefix.as_deref() {
        restore_args.push(prefix.as_os_str().to_os_string());
    } else {
        restore_args.push(OsString::from("."));
    }

    run_git_for_status(repo_root.as_path(), restore_args, None)?;
    Ok(())
}

/// Returns the default author and committer identity for ghost commits.
fn default_commit_identity() -> Vec<(OsString, OsString)> {
    vec![
        (
            OsString::from("GIT_AUTHOR_NAME"),
            OsString::from("Code Snapshot"),
        ),
        (
            OsString::from("GIT_AUTHOR_EMAIL"),
            OsString::from("snapshot@code.local"),
        ),
        (
            OsString::from("GIT_COMMITTER_NAME"),
            OsString::from("Code Snapshot"),
        ),
        (
            OsString::from("GIT_COMMITTER_EMAIL"),
            OsString::from("snapshot@code.local"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::run_git_for_stdout;
    use pretty_assertions::assert_eq;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Runs a git command in the test repository and asserts success.
    fn run_git_in(repo_path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    }

    /// Runs a git command and returns its trimmed stdout output.
    fn run_git_stdout(repo_path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .output()
            .expect("git command");
        assert!(output.status.success(), "git command failed: {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn ghost_index_tempdir_uses_home_tmp_when_available() -> Result<(), GitToolingError> {
        let home = tempfile::tempdir()?;
        let index_tempdir = create_index_tempdir(Some(home.path()))?;

        assert!(index_tempdir.path().starts_with(home.path().join("tmp")));
        Ok(())
    }

    /// Initializes a repository with consistent settings for cross-platform tests.
    fn init_test_repo(repo: &Path) {
        let init_status = Command::new("git")
            .current_dir(repo)
            .args(["init", "--initial-branch=main"])
            .output()
            .expect("git command");

        if !init_status.status.success() {
            let fallback = Command::new("git")
                .current_dir(repo)
                .arg("init")
                .status()
                .expect("git command");
            assert!(fallback.success(), "git init failed without --initial-branch");

            let set_head = Command::new("git")
                .current_dir(repo)
                .args(["symbolic-ref", "HEAD", "refs/heads/main"])
                .status()
                .expect("git command");
            assert!(
                set_head.success(),
                "git symbolic-ref HEAD refs/heads/main failed"
            );
        }

        run_git_in(repo, &["config", "core.autocrlf", "false"]);
    }

    #[test]
    fn repository_discovery_resolves_the_repo_containing_a_nested_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("workspace").join("nested-repo");
        let cwd = repo.join("src").join("deeper");
        std::fs::create_dir_all(&cwd).expect("nested cwd");
        init_test_repo(&repo);

        let roots = discover_repository_roots(&cwd).expect("discover containing repository");

        assert_eq!(roots, vec![repo.canonicalize().expect("canonical repo")]);
    }

    #[test]
    fn repository_discovery_finds_a_repo_below_a_non_git_workspace() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let repo = workspace.join("child").join("repo");
        std::fs::create_dir_all(&repo).expect("repository directory");
        init_test_repo(&repo);

        let roots = discover_repository_roots(&workspace).expect("discover nested repository");

        assert_eq!(roots, vec![repo.canonicalize().expect("canonical repo")]);
    }

    #[test]
    fn repository_discovery_returns_all_nested_repos_in_stable_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let repo_b = workspace.join("b-repo");
        let repo_a = workspace.join("a-repo");
        std::fs::create_dir_all(&repo_a).expect("first repository directory");
        std::fs::create_dir_all(&repo_b).expect("second repository directory");
        init_test_repo(&repo_b);
        init_test_repo(&repo_a);

        let roots = discover_repository_roots(&workspace).expect("discover nested repositories");

        assert_eq!(
            roots,
            vec![
                repo_a.canonicalize().expect("canonical first repo"),
                repo_b.canonicalize().expect("canonical second repo"),
            ]
        );
    }

    #[test]
    fn bounded_repository_discovery_does_not_scan_beyond_configured_depth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let shallow = workspace.join("shallow");
        let deep = workspace.join("level-one").join("level-two").join("deep");
        std::fs::create_dir_all(&shallow).expect("shallow repository directory");
        std::fs::create_dir_all(&deep).expect("deep repository directory");
        init_test_repo(&shallow);
        init_test_repo(&deep);

        let roots = discover_repository_roots_with_depth(&workspace, 1)
            .expect("discover repositories within one directory level");

        assert_eq!(
            roots,
            vec![shallow.canonicalize().expect("canonical shallow repo")]
        );
    }

    #[test]
    fn zero_depth_repository_discovery_does_not_scan_sibling_or_child_repos() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let child = workspace.join("child");
        std::fs::create_dir_all(&child).expect("child repository directory");
        init_test_repo(&child);

        let err = discover_repository_roots_with_depth(&workspace, 0).unwrap_err();

        assert!(matches!(err, GitToolingError::NotAGitRepository { .. }));
    }

    #[test]
    /// Verifies a ghost commit can be created and restored end to end.
    fn create_and_restore_roundtrip() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);
        std::fs::write(repo.join("tracked.txt"), "initial\n")?;
        std::fs::write(repo.join("delete-me.txt"), "to be removed\n")?;
        run_git_in(repo, &["add", "tracked.txt", "delete-me.txt"]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        );

        let tracked_contents = "modified contents\n";
        std::fs::write(repo.join("tracked.txt"), tracked_contents)?;
        std::fs::remove_file(repo.join("delete-me.txt"))?;
        let new_file_contents = "hello ghost\n";
        std::fs::write(repo.join("new-file.txt"), new_file_contents)?;
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n")?;
        let ignored_contents = "ignored but captured\n";
        std::fs::write(repo.join("ignored.txt"), ignored_contents)?;

        let options =
            CreateGhostCommitOptions::new(repo).force_include(vec![PathBuf::from("ignored.txt")]);
        let ghost = create_ghost_commit(&options)?;

        assert!(ghost.parent().is_some());
        let cat = run_git_for_stdout(
            repo,
            vec![
                OsString::from("show"),
                OsString::from(format!("{}:ignored.txt", ghost.id())),
            ],
            None,
        )?;
        assert_eq!(cat, ignored_contents.trim());

        std::fs::write(repo.join("tracked.txt"), "other state\n")?;
        std::fs::write(repo.join("ignored.txt"), "changed\n")?;
        std::fs::remove_file(repo.join("new-file.txt"))?;
        std::fs::write(repo.join("ephemeral.txt"), "temp data\n")?;

        restore_ghost_commit(repo, &ghost)?;

        let tracked_after = std::fs::read_to_string(repo.join("tracked.txt"))?;
        assert_eq!(tracked_after, tracked_contents);
        let ignored_after = std::fs::read_to_string(repo.join("ignored.txt"))?;
        assert_eq!(ignored_after, ignored_contents);
        let new_file_after = std::fs::read_to_string(repo.join("new-file.txt"))?;
        assert_eq!(new_file_after, new_file_contents);
        assert_eq!(repo.join("delete-me.txt").exists(), false);
        assert!(repo.join("ephemeral.txt").exists());

        Ok(())
    }

    #[test]
    fn large_repository_snapshot_only_stages_changed_paths() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);
        for index in 0..1_500 {
            std::fs::write(repo.join(format!("tracked-{index:04}.txt")), "unchanged\n")?;
        }
        run_git_in(repo, &["add", "."]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "large baseline",
            ],
        );
        std::fs::write(repo.join("tracked-1499.txt"), "changed\n")?;

        let paths = capture_changed_paths(repo, None, Some(Duration::from_secs(10)))?;

        assert_eq!(paths, vec![PathBuf::from("tracked-1499.txt")]);
        let ghost = create_ghost_commit(
            &CreateGhostCommitOptions::new(repo).command_timeout(Duration::from_secs(10)),
        )?;
        assert!(!ghost.id().is_empty());
        Ok(())
    }

    #[test]
    /// Ensures ghost commits succeed in repositories without an existing HEAD.
    fn create_snapshot_without_existing_head() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        let tracked_contents = "first contents\n";
        std::fs::write(repo.join("tracked.txt"), tracked_contents)?;
        let ignored_contents = "ignored but captured\n";
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n")?;
        std::fs::write(repo.join("ignored.txt"), ignored_contents)?;

        let options =
            CreateGhostCommitOptions::new(repo).force_include(vec![PathBuf::from("ignored.txt")]);
        let ghost = create_ghost_commit(&options)?;

        assert!(ghost.parent().is_none());

        let message = run_git_stdout(repo, &["log", "-1", "--format=%s", ghost.id()]);
        assert_eq!(message, DEFAULT_COMMIT_MESSAGE);

        let ignored = run_git_stdout(repo, &["show", &format!("{}:ignored.txt", ghost.id())]);
        assert_eq!(ignored, ignored_contents.trim());

        Ok(())
    }

    #[test]
    /// Confirms custom messages are used when creating ghost commits.
    fn create_ghost_commit_uses_custom_message() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        std::fs::write(repo.join("tracked.txt"), "contents\n")?;
        run_git_in(repo, &["add", "tracked.txt"]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        let message = "custom message";
        let ghost = create_ghost_commit(&CreateGhostCommitOptions::new(repo).message(message))?;
        let commit_message = run_git_stdout(repo, &["log", "-1", "--format=%s", ghost.id()]);
        assert_eq!(commit_message, message);

        Ok(())
    }

    #[test]
    /// Verifies the optional post-commit hook runs after snapshot creation.
    fn post_commit_hook_runs_after_creation() -> Result<(), GitToolingError> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);

        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        std::fs::write(repo.join("tracked.txt"), "contents\n")?;
        run_git_in(repo, &["add", "tracked.txt"]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        fn hook() {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let options = CreateGhostCommitOptions::new(repo).post_commit_hook(&hook);
        let _ = create_ghost_commit(&options)?;
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    /// Rejects force-included paths that escape the repository.
    fn create_ghost_commit_rejects_force_include_parent_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        init_test_repo(repo);
        let options = CreateGhostCommitOptions::new(repo)
            .force_include(vec![PathBuf::from("../outside.txt")]);
        let err = create_ghost_commit(&options).unwrap_err();
        assert!(matches!(err, GitToolingError::PathEscapesRepository { .. }));
    }

    #[test]
    /// Restoring a ghost commit from a non-git directory fails.
    fn restore_requires_git_repository() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = restore_to_commit(temp.path(), "deadbeef").unwrap_err();
        assert!(matches!(err, GitToolingError::NotAGitRepository { .. }));
    }

    #[test]
    /// Restoring from a subdirectory affects only that subdirectory.
    fn restore_from_subdirectory_restores_files_relatively() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        std::fs::create_dir_all(repo.join("workspace"))?;
        let workspace = repo.join("workspace");
        std::fs::write(repo.join("root.txt"), "root contents\n")?;
        std::fs::write(workspace.join("nested.txt"), "nested contents\n")?;
        run_git_in(repo, &["add", "."]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        std::fs::write(repo.join("root.txt"), "root modified\n")?;
        std::fs::write(workspace.join("nested.txt"), "nested modified\n")?;

        let ghost = create_ghost_commit(&CreateGhostCommitOptions::new(&workspace))?;

        std::fs::write(repo.join("root.txt"), "root after\n")?;
        std::fs::write(workspace.join("nested.txt"), "nested after\n")?;

        restore_ghost_commit(&workspace, &ghost)?;

        let root_after = std::fs::read_to_string(repo.join("root.txt"))?;
        assert_eq!(root_after, "root after\n");
        let nested_after = std::fs::read_to_string(workspace.join("nested.txt"))?;
        assert_eq!(nested_after, "nested modified\n");
        assert!(!workspace.join("code-rs").exists());

        Ok(())
    }

    #[test]
    /// Restoring from a subdirectory preserves ignored files in parent folders.
    fn restore_from_subdirectory_preserves_parent_vscode() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        let workspace = repo.join("code-rs");
        std::fs::create_dir_all(&workspace)?;
        std::fs::write(repo.join(".gitignore"), ".vscode/\n")?;
        std::fs::write(workspace.join("tracked.txt"), "snapshot version\n")?;
        run_git_in(repo, &["add", "."]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        std::fs::write(workspace.join("tracked.txt"), "snapshot delta\n")?;
        let ghost = create_ghost_commit(&CreateGhostCommitOptions::new(&workspace))?;

        std::fs::write(workspace.join("tracked.txt"), "post-snapshot\n")?;
        let vscode = repo.join(".vscode");
        std::fs::create_dir_all(&vscode)?;
        std::fs::write(vscode.join("settings.json"), "{\n  \"after\": true\n}\n")?;

        restore_ghost_commit(&workspace, &ghost)?;

        let tracked_after = std::fs::read_to_string(workspace.join("tracked.txt"))?;
        assert_eq!(tracked_after, "snapshot delta\n");
        assert!(vscode.join("settings.json").exists());
        let settings_after = std::fs::read_to_string(vscode.join("settings.json"))?;
        assert_eq!(settings_after, "{\n  \"after\": true\n}\n");

        Ok(())
    }

    #[test]
    /// Restoring from the repository root keeps ignored files intact.
    fn restore_preserves_ignored_files() -> Result<(), GitToolingError> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);

        std::fs::write(repo.join(".gitignore"), ".vscode/\n")?;
        std::fs::write(repo.join("tracked.txt"), "snapshot version\n")?;
        let vscode = repo.join(".vscode");
        std::fs::create_dir_all(&vscode)?;
        std::fs::write(vscode.join("settings.json"), "{\n  \"before\": true\n}\n")?;
        run_git_in(repo, &["add", ".gitignore", "tracked.txt"]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        std::fs::write(repo.join("tracked.txt"), "snapshot delta\n")?;
        let ghost = create_ghost_commit(&CreateGhostCommitOptions::new(repo))?;

        std::fs::write(repo.join("tracked.txt"), "post-snapshot\n")?;
        std::fs::write(vscode.join("settings.json"), "{\n  \"after\": true\n}\n")?;
        std::fs::write(repo.join("temp.txt"), "new file\n")?;

        restore_ghost_commit(repo, &ghost)?;

        let tracked_after = std::fs::read_to_string(repo.join("tracked.txt"))?;
        assert_eq!(tracked_after, "snapshot delta\n");
        assert!(vscode.join("settings.json").exists());
        let settings_after = std::fs::read_to_string(vscode.join("settings.json"))?;
        assert_eq!(settings_after, "{\n  \"after\": true\n}\n");
        assert!(repo.join("temp.txt").exists());

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ghost_commit_timeout_terminates_git_and_filter_processes() -> Result<(), GitToolingError> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let repo = temp.path();
        init_test_repo(repo);
        std::fs::write(repo.join("tracked.txt"), "initial\n")?;
        run_git_in(repo, &["add", "tracked.txt"]);
        run_git_in(
            repo,
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );

        let pid_file = repo.join("filter.pid");
        let filter = repo.join("slow-filter.sh");
        std::fs::write(
            &filter,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 30\ncat\n",
                pid_file.display()
            ),
        )?;
        let mut permissions = std::fs::metadata(&filter)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&filter, permissions)?;
        run_git_in(
            repo,
            &[
                "config",
                "filter.slow.clean",
                filter.to_str().expect("UTF-8 filter path"),
            ],
        );
        std::fs::write(repo.join(".gitattributes"), "*.slow filter=slow\n")?;
        std::fs::write(repo.join("blocked.slow"), "content\n")?;

        let started = Instant::now();
        let error = create_ghost_commit(
            &CreateGhostCommitOptions::new(repo).command_timeout(Duration::from_millis(250)),
        )
        .expect_err("slow git add should time out");

        assert!(
            matches!(&error, GitToolingError::Io(io_error) if io_error.kind() == std::io::ErrorKind::TimedOut),
            "unexpected error: {error}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let filter_pid = std::fs::read_to_string(&pid_file)?;
        let status = Command::new("kill")
            .args(["-0", filter_pid.trim()])
            .status()?;
        assert!(!status.success(), "slow filter process survived snapshot timeout");
        Ok(())
    }
}
