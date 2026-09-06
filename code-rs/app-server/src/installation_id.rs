use std::fs::OpenOptions;
use std::io::Read;
use std::io::Result;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use uuid::Uuid;

const INSTALLATION_ID_FILENAME: &str = "installation_id";

pub(crate) async fn resolve_installation_id(code_home: &Path) -> Result<String> {
    tokio::fs::create_dir_all(code_home).await?;
    let path = code_home.join(INSTALLATION_ID_FILENAME);
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);

        let mut file = options.open(path)?;
        file.lock()?;

        #[cfg(unix)]
        {
            let metadata = file.metadata()?;
            let current_mode = metadata.permissions().mode() & 0o777;
            if current_mode != 0o600 {
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o600);
                file.set_permissions(permissions)?;
            }
        }

        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        if let Ok(existing) = Uuid::parse_str(contents.trim()) {
            return Ok(existing.to_string());
        }

        let installation_id = Uuid::now_v7().to_string();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(installation_id.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(installation_id)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn installation_id_is_stable_and_rewrites_invalid_contents() {
        let code_home = tempfile::tempdir().expect("create code home");
        let path = code_home.path().join(INSTALLATION_ID_FILENAME);

        tokio::fs::write(&path, "not-a-uuid")
            .await
            .expect("write invalid installation id");
        let first = resolve_installation_id(code_home.path())
            .await
            .expect("resolve installation id");
        let second = resolve_installation_id(code_home.path())
            .await
            .expect("reuse installation id");

        assert_eq!(first, second);
        assert!(Uuid::parse_str(&first).is_ok());
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read installation id"),
            first
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installation_id_file_is_private() {
        let code_home = tempfile::tempdir().expect("create code home");
        resolve_installation_id(code_home.path())
            .await
            .expect("resolve installation id");

        let mode = tokio::fs::metadata(code_home.path().join(INSTALLATION_ID_FILENAME))
            .await
            .expect("read installation id metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
