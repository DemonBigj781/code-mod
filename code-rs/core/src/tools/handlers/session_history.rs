use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;
use code_session_history::{Action, Error, Request, Settings};
use serde_json::json;
use std::path::Path;

use crate::codex::Session;
use crate::protocol::SandboxPolicy;
use crate::tools::context::{ToolInvocation, ToolPayload};
use crate::tools::events::execute_custom_tool;
use crate::tools::handlers::{tool_error, tool_output};
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;

pub(crate) struct SessionHistoryHandler;

#[async_trait]
impl ToolHandler for SessionHistoryHandler {
    // Imports mutate shared state, so use the default exclusive scheduling.
    async fn handle(
        &self,
        sess: &Session,
        _tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return history_error(
                inv.ctx.call_id,
                "invalid_argument",
                "session_history expects JSON function arguments",
            );
        };
        let request: Request = match serde_json::from_str(&arguments) {
            Ok(request) => request,
            Err(error) => {
                return history_error(
                    inv.ctx.call_id,
                    "invalid_argument",
                    format!("invalid session_history arguments: {error}"),
                );
            }
        };
        let code_home = sess.code_home().to_path_buf();
        let cwd = sess.get_cwd().to_path_buf();
        let policy = sess.get_sandbox_policy().clone();
        let params = serde_json::from_str(&arguments).ok();
        let call_id = inv.ctx.call_id.clone();
        execute_custom_tool(
            sess,
            &inv.ctx,
            crate::openai_tools::SESSION_HISTORY_TOOL_NAME.to_owned(),
            params,
            move || async move {
                let result = tokio::task::spawn_blocking(move || {
                    let settings = Settings::discover(&code_home);
                    if request.action == Action::ImportOpenaiExport {
                        let target = settings.data_dir.canonicalize().map_err(|error| Error {
                            code: "source_error",
                            message: format!("Import directory unavailable; stage the export in the list_stores inbox first: {error}"),
                        })?;
                        if !import_allowed(&policy, &cwd, &target) {
                            return Err(Error {
                                code: "permission_denied",
                                message: "Export import requires write permission for the session-history data directory; no files were changed.".into(),
                            });
                        }
                    }
                    code_session_history::execute(settings, request)
                }).await;
                match result {
                    Ok(Ok(mut value)) => {
                        value["ok"] = json!(true);
                        tool_output(call_id, value.to_string())
                    }
                    Ok(Err(error)) => history_error(call_id, error.code, error.message),
                    Err(error) => history_error(call_id, "internal_error", format!("session-history worker failed: {error}")),
                }
            },
        ).await
    }
}

fn import_allowed(policy: &SandboxPolicy, cwd: &Path, target: &Path) -> bool {
    policy.has_full_disk_write_access()
        || policy
            .get_writable_roots_with_cwd(cwd)
            .iter()
            .any(|root| root.is_path_writable(target))
}

fn history_error(call_id: String, code: &str, message: impl Into<String>) -> ResponseInputItem {
    let message: String = message.into();
    tool_error(
        call_id,
        code_secrets::redact_json(&json!({
            "ok": false, "error": { "code": code, "message": message }
        }))
        .to_string(),
    )
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn imports_respect_read_only_and_workspace_boundaries() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside workspace");
        let inside = workspace.path().join("history");
        let restricted = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
            allow_git_writes: false,
        };
        assert!(!import_allowed(
            &SandboxPolicy::ReadOnly,
            workspace.path(),
            &inside
        ));
        assert!(import_allowed(&restricted, workspace.path(), &inside));
        assert!(!import_allowed(
            &restricted,
            workspace.path(),
            outside.path()
        ));
        assert!(import_allowed(
            &SandboxPolicy::DangerFullAccess,
            workspace.path(),
            outside.path()
        ));
    }
}
