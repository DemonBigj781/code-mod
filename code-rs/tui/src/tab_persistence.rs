use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Preserve placeholders as well as their payloads, not just the visible text.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ComposerDraft {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) pastes: Vec<(String, String)>,
    pub(crate) images: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TabLayout {
    pub(crate) sessions: Vec<Uuid>,
    pub(crate) active: Option<Uuid>,
}

pub(crate) struct TabPersistence {
    root: PathBuf,
    layout_path: PathBuf,
    pub(crate) previous: Option<TabLayout>,
    pub(crate) layout_load_error: Option<String>,
    drafts: BTreeMap<Uuid, ComposerDraft>,
    last_layout: Option<TabLayout>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing draft directory"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    // NamedTempFile is private (0600 on Unix), and persist atomically replaces the target.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl TabPersistence {
    pub(crate) fn new(code_home: &Path, cwd: &Path) -> Self {
        let root = code_home.join("tui-tabs");
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_owned());
        let key = Sha256::digest(cwd.as_os_str().as_encoded_bytes());
        let layout_path = root.join(format!("layout-{key:x}.json"));
        Self {
            root,
            layout_path,
            previous: None,
            layout_load_error: None,
            drafts: BTreeMap::new(),
            last_layout: None,
        }
    }

    pub(crate) fn load_previous(&mut self) -> io::Result<()> {
        match read_json(&self.layout_path) {
            Ok(layout) => {
                self.previous = layout;
                self.layout_load_error = None;
                Ok(())
            }
            Err(error) => {
                self.layout_load_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    pub(crate) fn load_draft(&mut self, id: Uuid) -> io::Result<ComposerDraft> {
        let draft: ComposerDraft =
            read_json(&self.root.join("drafts").join(format!("{id}.json")))?.unwrap_or_default();
        self.drafts.insert(id, draft.clone());
        Ok(draft)
    }

    pub(crate) fn has_loaded_draft(&self, id: Uuid) -> bool {
        self.drafts.contains_key(&id)
    }

    pub(crate) fn save_draft(&mut self, id: Uuid, draft: ComposerDraft) -> io::Result<()> {
        if self.drafts.get(&id) == Some(&draft) {
            return Ok(());
        }
        let path = self.root.join("drafts").join(format!("{id}.json"));
        // Empty drafts are persisted too: submitting/clearing must not resurrect old text.
        write_json(&path, &draft)?;
        self.drafts.insert(id, draft);
        Ok(())
    }

    pub(crate) fn save_layout(&mut self, layout: TabLayout) -> io::Result<()> {
        if let Some(error) = &self.layout_load_error {
            return Err(io::Error::other(format!(
                "Previous tab layout could not be read; original kept: {error}"
            )));
        }
        if layout.sessions.is_empty() || self.last_layout.as_ref() == Some(&layout) {
            return Ok(());
        }
        write_json(&self.layout_path, &layout)?;
        self.last_layout = Some(layout);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopening_restores_separate_drafts_and_previous_layout() {
        let dir = tempfile::tempdir().unwrap();
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let draft = ComposerDraft {
            text: "hello 界\n[paste] [image]".into(),
            cursor: 6,
            pastes: vec![("[paste]".into(), "long text\n".repeat(500))],
            images: BTreeMap::from([("[image]".into(), PathBuf::from("/saved/image.png"))]),
        };
        let layout = TabLayout {
            sessions: vec![b, a],
            active: Some(a),
        };
        let mut store = TabPersistence::new(dir.path(), dir.path());
        store.save_draft(a, draft.clone()).unwrap();
        store
            .save_draft(
                b,
                ComposerDraft {
                    text: "other".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store.save_layout(layout.clone()).unwrap();
        drop(store);
        let mut reopened = TabPersistence::new(dir.path(), dir.path());
        reopened.load_previous().unwrap();
        assert_eq!(reopened.previous, Some(layout));
        assert_eq!(reopened.load_draft(a).unwrap(), draft);
        assert_eq!(reopened.load_draft(b).unwrap().text, "other");
        reopened.save_draft(a, ComposerDraft::default()).unwrap();
        let mut next = TabPersistence::new(dir.path(), dir.path());
        assert_eq!(next.load_draft(a).unwrap(), ComposerDraft::default());
    }

    #[test]
    fn current_layout_does_not_replace_in_memory_previous_or_another_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TabPersistence::new(dir.path(), Path::new("/first"));
        let previous = TabLayout {
            sessions: vec![Uuid::from_u128(1)],
            active: None,
        };
        store.save_layout(previous.clone()).unwrap();
        store.load_previous().unwrap();
        store
            .save_layout(TabLayout {
                sessions: vec![Uuid::from_u128(2)],
                active: None,
            })
            .unwrap();
        assert_eq!(store.previous, Some(previous));
        let mut other = TabPersistence::new(dir.path(), Path::new("/second"));
        other.load_previous().unwrap();
        assert!(other.previous.is_none());
    }

    #[test]
    fn corrupt_draft_is_reported_not_silently_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::from_u128(1);
        let mut store = TabPersistence::new(dir.path(), dir.path());
        store
            .save_draft(
                id,
                ComposerDraft {
                    text: "keep".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let path = store.root.join("drafts").join(format!("{id}.json"));
        std::fs::write(&path, b"broken").unwrap();
        let mut reopened = TabPersistence::new(dir.path(), dir.path());
        assert!(reopened.load_draft(id).is_err());
        assert!(!reopened.has_loaded_draft(id));
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    }

    #[test]
    fn corrupt_layout_is_reported_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TabPersistence::new(dir.path(), dir.path());
        let layout = TabLayout {
            sessions: vec![Uuid::from_u128(1)],
            active: None,
        };
        store.save_layout(layout.clone()).unwrap();
        std::fs::write(&store.layout_path, b"broken").unwrap();
        assert!(store.load_previous().is_err());
        assert!(store.save_layout(layout).is_err());
        assert_eq!(std::fs::read(&store.layout_path).unwrap(), b"broken");
    }

    #[test]
    #[cfg(unix)]
    fn saved_inputs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::from_u128(1);
        let mut store = TabPersistence::new(dir.path(), dir.path());
        store.save_draft(id, ComposerDraft::default()).unwrap();
        let path = store.root.join("drafts").join(format!("{id}.json"));
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
