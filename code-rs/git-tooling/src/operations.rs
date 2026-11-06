use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::GitToolingError;

pub(crate) fn ensure_git_repository(path: &Path) -> Result<(), GitToolingError> {
    ensure_git_repository_with_timeout(path, None)
}

pub(crate) fn ensure_git_repository_with_timeout(
    path: &Path,
    timeout: Option<Duration>,
) -> Result<(), GitToolingError> {
    match run_git_for_stdout_with_timeout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--is-inside-work-tree"),
        ],
        None,
        timeout,
    ) {
        Ok(output) if output.trim() == "true" => Ok(()),
        Ok(_) => Err(GitToolingError::NotAGitRepository {
            path: path.to_path_buf(),
        }),
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(128) => {
            Err(GitToolingError::NotAGitRepository {
                path: path.to_path_buf(),
            })
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn resolve_head_with_timeout(
    path: &Path,
    timeout: Option<Duration>,
) -> Result<Option<String>, GitToolingError> {
    match run_git_for_stdout_with_timeout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("HEAD"),
        ],
        None,
        timeout,
    ) {
        Ok(sha) => Ok(Some(sha)),
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(128) => Ok(None),
        Err(other) => Err(other),
    }
}

pub(crate) fn normalize_relative_path(path: &Path) -> Result<PathBuf, GitToolingError> {
    let mut result = PathBuf::new();
    let mut saw_component = false;
    for component in path.components() {
        saw_component = true;
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(GitToolingError::PathEscapesRepository {
                        path: path.to_path_buf(),
                    });
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(GitToolingError::NonRelativePath {
                    path: path.to_path_buf(),
                });
            }
        }
    }

    if !saw_component {
        return Err(GitToolingError::NonRelativePath {
            path: path.to_path_buf(),
        });
    }

    Ok(result)
}

pub(crate) fn resolve_repository_root(path: &Path) -> Result<PathBuf, GitToolingError> {
    resolve_repository_root_with_timeout(path, None)
}

pub(crate) fn resolve_repository_root_with_timeout(
    path: &Path,
    timeout: Option<Duration>,
) -> Result<PathBuf, GitToolingError> {
    let root = run_git_for_stdout_with_timeout(
        path,
        vec![
            OsString::from("rev-parse"),
            OsString::from("--show-toplevel"),
        ],
        None,
        timeout,
    )?;
    Ok(PathBuf::from(root))
}

pub(crate) fn apply_repo_prefix_to_force_include(
    prefix: Option<&Path>,
    paths: &[PathBuf],
) -> Vec<PathBuf> {
    if paths.is_empty() {
        return Vec::new();
    }

    match prefix {
        Some(prefix) => paths.iter().map(|path| prefix.join(path)).collect(),
        None => paths.to_vec(),
    }
}

pub(crate) fn repo_subdir(repo_root: &Path, repo_path: &Path) -> Option<PathBuf> {
    if repo_root == repo_path {
        return None;
    }

    repo_path
        .strip_prefix(repo_root)
        .ok()
        .and_then(non_empty_path)
        .or_else(|| {
            let repo_root_canon = repo_root.canonicalize().ok()?;
            let repo_path_canon = repo_path.canonicalize().ok()?;
            repo_path_canon
                .strip_prefix(&repo_root_canon)
                .ok()
                .and_then(non_empty_path)
        })
}

fn non_empty_path(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path.to_path_buf())
    }
}

pub(crate) fn run_git_for_status<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<(), GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git(dir, args, env, None)?;
    Ok(())
}

pub(crate) fn run_git_for_status_with_timeout<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
    timeout: Option<Duration>,
) -> Result<(), GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git(dir, args, env, timeout)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn run_git_for_stdout<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
) -> Result<String, GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git_for_stdout_with_timeout(dir, args, env, None)
}

pub(crate) fn run_git_for_stdout_with_timeout<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
    timeout: Option<Duration>,
) -> Result<String, GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let run = run_git(dir, args, env, timeout)?;
    String::from_utf8(run.output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|source| GitToolingError::GitOutputUtf8 {
            command: run.command,
            source,
        })
}

fn run_git<I, S>(
    dir: &Path,
    args: I,
    env: Option<&[(OsString, OsString)]>,
    timeout: Option<Duration>,
) -> Result<GitRun, GitToolingError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let iterator = args.into_iter();
    let (lower, upper) = iterator.size_hint();
    let mut args_vec = Vec::with_capacity(upper.unwrap_or(lower));
    for arg in iterator {
        args_vec.push(OsString::from(arg.as_ref()));
    }
    let command_string = build_command_string(&args_vec);
    let mut command = Command::new("git");
    command.current_dir(dir);
    if let Some(envs) = env {
        for (key, value) in envs {
            command.env(key, value);
        }
    }
    command.args(&args_vec);
    let output = match timeout {
        Some(timeout) => run_command_with_timeout(&mut command, &command_string, timeout)?,
        None => command.output()?,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(GitToolingError::GitCommand {
            command: command_string,
            status: output.status,
            stderr,
        });
    }
    Ok(GitRun {
        command: command_string,
        output,
    })
}

fn run_command_with_timeout(
    command: &mut Command,
    command_string: &str,
    timeout: Duration,
) -> Result<std::process::Output, GitToolingError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::other(format!("failed to capture stdout for `{command_string}`"))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        std::io::Error::other(format!("failed to capture stderr for `{command_string}`"))
    })?;
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_child_tree(&mut child);
            let _ = join_reader(stdout_reader, command_string, "stdout")?;
            let _ = join_reader(stderr_reader, command_string, "stderr")?;
            return Err(GitToolingError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("git command `{command_string}` exceeded {timeout:?}"),
            )));
        }
        thread::sleep(Duration::from_millis(10));
    };

    Ok(std::process::Output {
        status,
        stdout: join_reader(stdout_reader, command_string, "stdout")?,
        stderr: join_reader(stderr_reader, command_string, "stderr")?,
    })
}

fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    command: &str,
    stream: &str,
) -> Result<Vec<u8>, GitToolingError> {
    reader
        .join()
        .map_err(|_| {
            GitToolingError::Io(std::io::Error::other(format!(
                "{stream} reader panicked for `{command}`"
            )))
        })?
        .map_err(GitToolingError::Io)
}

fn terminate_child_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let process_group = -(child.id() as i32);
        let _ = libc::kill(process_group, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn build_command_string(args: &[OsString]) -> String {
    if args.is_empty() {
        return "git".to_owned();
    }
    let joined = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    format!("git {joined}")
}

struct GitRun {
    command: String,
    output: std::process::Output,
}
