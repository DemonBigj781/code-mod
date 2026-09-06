use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs;

use crate::process;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonSettings {
    pub remote_control_enabled: bool,
}

impl DaemonSettings {
    pub async fn load(path: &Path) -> Result<Self> {
        let contents = match fs::read_to_string(path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read daemon settings {}", path.display()));
            }
        };
        serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse daemon settings {}", path.display()))
    }

    pub async fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("daemon settings path has no parent")?;
        process::prepare_private_state_dir(parent).await?;
        let contents = serde_json::to_vec_pretty(self).context("failed to serialize settings")?;
        process::write_private_contents(path, &contents).await
    }
}
