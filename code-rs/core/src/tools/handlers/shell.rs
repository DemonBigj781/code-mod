use crate::codex::Session;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use crate::tools::handlers::tool_error;
use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;

pub(crate) struct ShellHandler;

#[async_trait]
impl ToolHandler for ShellHandler {
    async fn handle(
        &self,
        sess: &Session,
        turn_diff_tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        match inv.payload {
            ToolPayload::Function { arguments } => {
                let params = match crate::codex::exec_tool::parse_container_exec_arguments(
                    arguments,
                    sess,
                    inv.ctx.call_id.as_str(),
                ) {
                    Ok(params) => params,
                    Err(output) => return *output,
                };

                if crate::tools::handlers::ssh::is_shell_alias(&params.command) {
                    return crate::tools::handlers::ssh::handle_shell_alias(
                        sess,
                        turn_diff_tracker,
                        &inv.ctx,
                        code_protocol::models::ShellToolCallParams {
                            command: params.command,
                            workdir: None,
                            timeout_ms: params.timeout_ms,
                            sandbox_permissions: Some(params.sandbox_permissions),
                            prefix_rule: None,
                            additional_permissions: params.additional_permissions,
                            justification: params.justification,
                        },
                        inv.attempt_req,
                    )
                    .await;
                }

                crate::codex::exec_tool::handle_container_exec_with_params(
                    params,
                    sess,
                    turn_diff_tracker,
                    &inv.ctx,
                    inv.attempt_req,
                )
                .await
            }
            ToolPayload::LocalShell { params } => {
                let exec_params = crate::codex::exec_tool::to_exec_params(params, sess);
                crate::codex::exec_tool::handle_container_exec_with_params(
                    exec_params,
                    sess,
                    turn_diff_tracker,
                    &inv.ctx,
                    inv.attempt_req,
                )
                .await
            }
            _ => tool_error(
                inv.ctx.call_id.clone(),
                format!("unsupported shell payload for tool `{}`", inv.tool_name),
            ),
        }
    }
}
