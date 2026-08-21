use chrono::{DateTime, Utc};
use code_protocol::openai_models::ModelInfo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LEGACY_MODEL_CACHE_FILE: &str = "models_cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ModelsCache {
    pub(crate) fetched_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) etag: Option<String>,
    pub(crate) models: Vec<ModelInfo>,
}

pub(crate) fn is_fresh(fetched_at: DateTime<Utc>, ttl: Duration) -> bool {
    if ttl.is_zero() {
        return false;
    }
    let Ok(ttl_duration) = chrono::Duration::from_std(ttl) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(fetched_at);
    age <= ttl_duration
}

pub(crate) fn cache_path(code_home: &Path, provider_id: &str) -> PathBuf {
    if provider_id == "openai" {
        return code_home.join(LEGACY_MODEL_CACHE_FILE);
    }

    let readable = provider_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(48)
        .collect::<String>();
    let readable = readable.trim_matches('-');
    let readable = if readable.is_empty() {
        "provider"
    } else {
        readable
    };
    let digest = Sha256::digest(provider_id.as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    code_home.join(format!("models_cache-{readable}-{suffix}.json"))
}

pub(crate) fn load_cache(path: &Path) -> io::Result<Option<ModelsCache>> {
    match std::fs::read(path) {
        Ok(contents) => {
            let cache = serde_json::from_slice(&contents)
                .map_err(|err| io::Error::new(ErrorKind::InvalidData, err.to_string()))?;
            Ok(Some(cache))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub(crate) fn save_cache(path: &Path, cache: &ModelsCache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_vec_pretty(cache)
        .map_err(|err| io::Error::new(ErrorKind::InvalidData, err.to_string()))?;

    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}
