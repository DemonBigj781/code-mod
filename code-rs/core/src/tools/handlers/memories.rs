use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;

use crate::codex::Session;
use crate::tools::context::{ToolInvocation, ToolPayload};
use crate::tools::events::execute_custom_tool;
use crate::tools::handlers::{tool_error, tool_output};
use crate::tools::registry::{ToolHandler, ToolSchedulingHints};
use crate::turn_diff_tracker::TurnDiffTracker;

pub(crate) struct MemoriesHandler;

#[async_trait]
impl ToolHandler for MemoriesHandler {
    fn scheduling_hints(&self) -> ToolSchedulingHints {
        ToolSchedulingHints::pure_parallel()
    }

    async fn handle(
        &self,
        sess: &Session,
        _tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(
                inv.ctx.call_id,
                "memories expects JSON function arguments".to_owned(),
            );
        };
        let params = serde_json::from_str(&arguments).ok();
        let code_home = sess.code_home().to_path_buf();
        let call_id = inv.ctx.call_id.clone();
        execute_custom_tool(
            sess,
            &inv.ctx,
            crate::openai_tools::MEMORIES_TOOL_NAME.to_owned(),
            params,
            move || async move {
                match tokio::task::spawn_blocking(move || {
                    crate::memories::execute_tool(&code_home, &arguments)
                })
                .await
                {
                    Ok(Ok(value)) => tool_output(call_id, value.to_string()),
                    Ok(Err(value)) => tool_error(call_id, value.to_string()),
                    Err(error) => tool_error(call_id, format!("memories task failed: {error}")),
                }
            },
        )
        .await
    }
}
