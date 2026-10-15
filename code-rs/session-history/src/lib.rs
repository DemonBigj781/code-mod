//! Bounded, redacted access to local Code, Codex, and imported conversation history.

mod files;
mod import;
mod normalize;
mod repository;
mod settings;
mod sqlite;

pub use settings::{Settings, Store};

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ListStores,
    ListSessions,
    SearchMessages,
    GetSession,
    GetTurn,
    InspectStore,
    ImportOpenaiExport,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub action: Action,
    pub store_id: Option<String>,
    pub session_ref: Option<String>,
    pub turn_id: Option<String>,
    pub query: Option<String>,
    pub role: Option<String>,
    pub cwd_contains: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub archived: Option<bool>,
    pub source_kind: Option<String>,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
    pub max_chars: Option<usize>,
    #[serde(default = "yes")]
    pub include_tools: bool,
    #[serde(default)]
    pub deep: bool,
    pub file_name: Option<String>,
    pub dataset_name: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new("source_error", error.to_string())
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::new("source_error", error.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new("invalid_export", error.to_string())
    }
}
impl Error {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
type Result<T> = std::result::Result<T, Error>;

fn required<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str> {
    value
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::new("invalid_argument", format!("{field} is required")))
}

/// Blocking entry point. Call from a blocking worker, not a Tokio async executor thread.
pub fn execute(settings: Settings, request: Request) -> Result<Value> {
    execute_with_staging(settings, request, None)
}

/// Execute with a caller-owned import staging parent on the dataset filesystem.
/// A supervising process can remove this directory after a worker is killed.
pub fn execute_with_staging(settings: Settings, request: Request, staging_parent: Option<&std::path::Path>) -> Result<Value> {
    validate(&request)?;
    if request.action == Action::ImportOpenaiExport {
        return import::import_export(
            &settings,
            required(&request.file_name, "file_name")?,
            required(&request.dataset_name, "dataset_name")?,
            staging_parent,
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let repository = repository::Repository::discover(settings).await?;
        repository.execute(&request).await
    })
}

fn validate(request: &Request) -> Result<()> {
    let max = if request.action == Action::SearchMessages {
        100
    } else {
        200
    };
    if request.limit.is_some_and(|n| n == 0 || n > max) {
        return Err(Error::new(
            "limit_exceeded",
            format!("limit must be between 1 and {max}"),
        ));
    }
    if request.max_chars.is_some_and(|n| n == 0 || n > 200_000) {
        return Err(Error::new(
            "limit_exceeded",
            "max_chars must be between 1 and 200000",
        ));
    }
    if request
        .role
        .as_deref()
        .is_some_and(|role| !matches!(role, "user" | "assistant"))
    {
        return Err(Error::new(
            "invalid_argument",
            "role must be user or assistant",
        ));
    }
    if request
        .since_ms
        .zip(request.until_ms)
        .is_some_and(|(since, until)| since > until)
    {
        return Err(Error::new(
            "invalid_argument",
            "since_ms must not exceed until_ms",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
