//! Small append-only Markdown logs for diagnosing transport and TUI ordering.

use chrono::Local;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum MarkdownLogKind {
    Chat,
    TuiDraw,
    Process,
}

impl MarkdownLogKind {
    fn filename(self) -> &'static str {
        match self {
            Self::Chat => "chat.md",
            Self::TuiDraw => "tui-draw.md",
            Self::Process => "processes.md",
        }
    }
}

/// Append a timestamped, human-readable diagnostic entry under `<code_home>/logs`.
/// Logging failures are returned to the caller so diagnostics never silently
/// claim that an event was recorded.
pub fn append(code_home: &Path, kind: MarkdownLogKind, title: &str, details: &str) -> std::io::Result<()> {
    let dir = code_home.join("logs");
    fs::create_dir_all(&dir)?;
    let path = dir.join(kind.filename());
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "\n## {} — {}", Local::now().to_rfc3339(), title.trim())?;
    writeln!(file, "\n```text\n{}\n```", details.trim_end())?;
    Ok(())
}
