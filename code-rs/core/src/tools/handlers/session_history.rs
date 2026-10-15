use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;

use crate::codex::Session;
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
        if arguments.len() > crate::session_history_worker::MAX_INPUT {
            return history_error(
                inv.ctx.call_id,
                "limit_exceeded",
                "session-history request exceeds 1 MiB",
            );
        }
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
                let result =
                    crate::session_history_worker::execute(&code_home, &cwd, &policy, &arguments)
                        .await;
                if result["ok"] == true {
                    tool_output(call_id, result.to_string())
                } else {
                    tool_error(call_id, result.to_string())
                }
            },
        )
        .await
    }
}

fn history_error(call_id: String, code: &str, message: impl Into<String>) -> ResponseInputItem {
    let message: String = message.into();
    tool_error(
        call_id,
        crate::session_history_worker::error(code, message).to_string(),
    )
}

#[cfg(test)]
mod permission_tests {
    use crate::protocol::SandboxPolicy;
    use crate::session_history_worker::import_allowed;

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
