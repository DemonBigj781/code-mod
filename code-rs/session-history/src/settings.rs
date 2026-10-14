use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Store {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub stores: Vec<Store>,
    pub data_dir: PathBuf,
}

impl Settings {
    pub fn discover(code_home: &Path) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| code_home.to_owned());
        let defaults = !matches!(
            std::env::var("CODE_SESSION_INCLUDE_DEFAULTS")
                .unwrap_or_default()
                .to_lowercase()
                .as_str(),
            "0" | "false" | "no" | "off"
        );
        let mut candidates = Vec::new();
        if defaults {
            candidates.extend([
                home.join(".code"),
                home.join(".codex"),
                code_home.to_owned(),
            ]);
            if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
                candidates.push(codex_home.into());
            }
            if let Ok(entries) = std::fs::read_dir(&home) {
                let mut syncs: Vec<_> = entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(".code"))
                    .map(|entry| entry.path().join("sync"))
                    .collect();
                syncs.sort();
                candidates.extend(syncs);
            }
        }
        if let Some(roots) = std::env::var_os("CODE_SESSION_ROOTS") {
            candidates.extend(std::env::split_paths(&roots).map(|path| expand_home(path, &home)));
        }
        let mut seen = HashSet::new();
        let mut labels = HashSet::new();
        let mut stores = Vec::new();
        for path in candidates {
            let Ok(root) = path.canonicalize() else {
                continue;
            };
            if !root.is_dir() || !seen.insert(root.clone()) {
                continue;
            }
            let name = root.file_name().unwrap_or_default().to_string_lossy();
            let hash = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
            let mut id = match name.as_ref() {
                ".code" => "code".to_owned(),
                ".codex" => "codex".to_owned(),
                _ => format!("{}-{}", slug(&name), &hash[..8]),
            };
            if labels.contains(&id) {
                let base = id.clone();
                for width in (8..=64).step_by(2) {
                    id = format!("{base}-{}", &hash[..width]);
                    if !labels.contains(&id) {
                        break;
                    }
                }
            }
            labels.insert(id.clone());
            stores.push(Store { id, root });
        }
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        // Keep the existing importer directory so installed datasets need no migration.
        let data_dir = expand_home(
            std::env::var_os("CODE_SESSION_DATA_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| data_home.join("code-session-mcp")),
            &home,
        );
        let data_dir = if data_dir.is_absolute() {
            data_dir
        } else {
            std::env::current_dir().unwrap_or(home).join(data_dir)
        };
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);
        Self { stores, data_dir }
    }
}

fn expand_home(path: PathBuf, home: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map(|tail| home.join(tail))
        .unwrap_or(path)
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "store".to_owned()
    } else {
        out.to_owned()
    }
}
