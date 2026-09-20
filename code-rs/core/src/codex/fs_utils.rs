use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub(crate) const AGENT_ARTIFACTS_SUBDIR: &str = "artifacts/agents";
pub(crate) const ARTIFACT_OWNER_MARKER: &str = ".code-owned.json";

fn ensure_artifact_session_dir(
    code_home: &Path,
    session_id: uuid::Uuid,
) -> Result<PathBuf, String> {
    let session_dir = code_home
        .join(AGENT_ARTIFACTS_SUBDIR)
        .join(session_id.to_string());
    std::fs::create_dir_all(&session_dir).map_err(|e| {
        format!(
            "Failed to create artifact session dir {}: {e}",
            session_dir.display()
        )
    })?;

    let marker_path = session_dir.join(ARTIFACT_OWNER_MARKER);
    if !marker_path.exists() {
        let created_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let marker = format!("{{\"created_unix\":{created_unix}}}\n");
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&marker_path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                file.write_all(marker.as_bytes()).map_err(|e| {
                    format!(
                        "Failed to mark artifact session dir {}: {e}",
                        session_dir.display()
                    )
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "Failed to mark artifact session dir {}: {error}",
                    session_dir.display()
                ));
            }
        }
    }

    Ok(session_dir)
}

pub(super) fn ensure_agent_dir(
    code_home: &Path,
    session_id: uuid::Uuid,
    agent_id: &str,
) -> Result<PathBuf, String> {
    let safe_agent_id = crate::fs_sanitize::safe_path_component(agent_id, "agent");
    let dir = ensure_artifact_session_dir(code_home, session_id)?.join(safe_agent_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create agent dir {}: {}", dir.display(), e))?;
    Ok(dir)
}

pub(super) fn ensure_user_dir(
    code_home: &Path,
    session_id: uuid::Uuid,
) -> Result<PathBuf, String> {
    let dir = ensure_artifact_session_dir(code_home, session_id)?.join("users");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create user dir {}: {}", dir.display(), e))?;
    Ok(dir)
}

pub(super) fn write_agent_file(
    dir: &Path,
    filename: &str,
    content: &str,
) -> Result<PathBuf, String> {
    if filename.chars().any(|ch| matches!(ch, '/' | '\\' | '\0')) {
        return Err(format!("Refusing to write invalid filename: {filename}"));
    }
    let candidate = Path::new(filename);
    if candidate.is_absolute() || candidate.components().count() != 1 {
        return Err(format!("Refusing to write non-file component: {filename}"));
    }
    let Some(file_name) = candidate.file_name() else {
        return Err(format!("Refusing to write invalid filename: {filename}"));
    };
    let file_name = file_name.to_string_lossy();
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        return Err(format!("Refusing to write invalid filename: {filename}"));
    }

    let path = dir.join(file_name.as_ref());
    std::fs::write(&path, content)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(path)
}

/// Write an agent file and return its display path, or a formatted error string.
pub(super) fn write_agent_file_display(
    dir: &Path,
    filename: &str,
    content: &str,
) -> String {
    match write_agent_file(dir, filename, content) {
        Ok(p) => p.display().to_string(),
        Err(e) => e,
    }
}

pub(super) const UNKNOWN_ERROR: &str = "Unknown error";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[test]
    fn agent_artifacts_are_session_scoped_under_code_home() {
        let code_home = TempDir::new().expect("temporary code home");
        let session_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555")
            .expect("valid session id");

        let dir = ensure_agent_dir(code_home.path(), session_id, "agent-1")
            .expect("create agent artifact directory");

        assert_eq!(
            dir,
            code_home
                .path()
                .join("artifacts")
                .join("agents")
                .join(session_id.to_string())
                .join("agent-1")
        );
        assert!(
            code_home
                .path()
                .join("artifacts")
                .join("agents")
                .join(session_id.to_string())
                .join(".code-owned.json")
                .is_file(),
            "session artifact roots must be explicitly marked as Code-owned"
        );
    }
}
