use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;
use serde::Deserialize;

use crate::SessionCatalog;
use crate::codex::Session;
use crate::protocol::EventMsg;
use crate::tools::context::{ToolInvocation, ToolPayload};
use crate::tools::handlers::{tool_error, tool_output};
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;

pub(crate) struct SessionTitleHandler;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleArgs {
    title: String,
}

fn parse_title(arguments: &str) -> Result<String, String> {
    let args: TitleArgs = serde_json::from_str(arguments).map_err(|error| error.to_string())?;
    let title = args.title.trim();
    if title.is_empty() || title.chars().count() > 80 || title.chars().any(char::is_control) {
        return Err("title must contain 1-80 characters on a single line".to_owned());
    }
    Ok(title.to_owned())
}

#[async_trait]
impl ToolHandler for SessionTitleHandler {
    async fn handle(
        &self,
        sess: &Session,
        _tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(inv.ctx.call_id, "set_session_title expects JSON arguments");
        };
        let title = match parse_title(&arguments) {
            Ok(title) => title,
            Err(error) => return tool_error(inv.ctx.call_id, error),
        };
        // There is deliberately no caller-supplied session ID: a model may name
        // only the conversation executing this tool.
        let catalog = SessionCatalog::new(sess.code_home().to_path_buf());
        match catalog
            .set_nickname(sess.session_uuid(), Some(title.clone()))
            .await
        {
            Ok(true) => {
                sess.send_ordered_from_ctx(&inv.ctx, EventMsg::SessionTitleUpdated { title })
                    .await;
                tool_output(inv.ctx.call_id, "Session title updated")
            }
            Ok(false) => tool_error(inv.ctx.call_id, "Current session is not in the catalog yet"),
            Err(error) => tool_error(
                inv.ctx.call_id,
                format!("Could not save session title: {error}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_isolation_session_title_rejects_other_targets_and_invalid_labels() {
        assert!(parse_title(r#"{"title":"Other tab","session_id":"other"}"#).is_err());
        for title in [
            "".to_owned(),
            " \n ".to_owned(),
            "a\nb".to_owned(),
            "x".repeat(81),
        ] {
            assert!(parse_title(&serde_json::json!({"title": title}).to_string()).is_err());
        }
        assert_eq!(
            parse_title(r#"{"title":"  Fix tab isolation  "}"#).unwrap(),
            "Fix tab isolation"
        );
        assert_eq!(
            parse_title(r#"{"title":"Topic-aware session names"}"#).unwrap(),
            "Topic-aware session names"
        );
    }
}
