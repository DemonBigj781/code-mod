use crate::{Error, Result};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn allowed_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = path.canonicalize()?;
    if !path.starts_with(&root) {
        return Err(Error::new(
            "source_error",
            "source path is outside configured store root",
        ));
    }
    Ok(path)
}

pub(crate) fn open_regular(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(Error::new(
            "source_error",
            "source file must not be a symlink",
        ));
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new("source_error", "source must be a regular file"));
    }
    Ok(file)
}

pub(crate) fn visit_records(
    path: &Path,
    mut visit: impl FnMut(usize, Value) -> bool,
) -> Result<()> {
    let mut reader = BufReader::new(open_regular(path)?);
    let mut ordinal = 0;
    loop {
        let mut line = Vec::new();
        let size = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if size == 0 {
            break;
        }
        if size as u64 > MAX_RECORD_BYTES {
            return Err(Error::new(
                "limit_exceeded",
                "history record exceeds 16 MiB",
            ));
        }
        ordinal += 1;
        let text = String::from_utf8_lossy(&line);
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.is_object() && !visit(ordinal, value) {
            break;
        }
    }
    Ok(())
}

pub(crate) fn inspect(path: &Path, history: bool) -> Result<Value> {
    let mut total = 0usize;
    let mut malformed = 0usize;
    let mut valid = 0usize;
    let mut sessions = std::collections::HashSet::new();
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    let mut responses = std::collections::BTreeMap::<String, usize>::new();
    let mut reader = BufReader::new(open_regular(path)?);
    loop {
        let mut line = Vec::new();
        if reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        if line.len() as u64 > MAX_RECORD_BYTES {
            return Err(Error::new(
                "limit_exceeded",
                "history record exceeds 16 MiB",
            ));
        }
        total += 1;
        let Ok(record) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) else {
            malformed += 1;
            continue;
        };
        if !record.is_object() {
            continue;
        }
        if let Some(id) = record["session_id"].as_str() {
            valid += 1;
            sessions.insert(id.to_owned());
        }
        *kinds
            .entry(record["type"].as_str().unwrap_or("<none>").to_owned())
            .or_default() += 1;
        if record["type"] == "response_item" && record["payload"].is_object() {
            *responses
                .entry(
                    record["payload"]["type"]
                        .as_str()
                        .unwrap_or("<none>")
                        .to_owned(),
                )
                .or_default() += 1;
        }
    }
    Ok(if history {
        json!({"path":path,"total_lines":total,"malformed_lines":malformed,"valid_records":valid,"session_count":sessions.len()})
    } else {
        json!({"path":path,"total_lines":total,"malformed_lines":malformed,"record_types":kinds,"response_item_types":responses})
    })
}
