use crate::codex::Session;
use crate::codex::ToolCallCtx;
use crate::protocol::AskForApproval;
use crate::protocol::EventMsg;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::tool_error;
use crate::tools::handlers::tool_output;
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use async_trait::async_trait;
use code_protocol::models::ResponseInputItem;
use code_protocol::request_resources::RequestResourcesArgs;
use code_protocol::request_resources::RequestResourcesEvent;
use code_protocol::request_resources::RequestResourcesResponse;
use code_protocol::request_resources::ResourceGrantScope;
use code_protocol::request_resources::ResourceRequestProfile;

pub(crate) struct RequestResourcesHandler;

#[async_trait]
impl ToolHandler for RequestResourcesHandler {
    async fn handle(
        &self,
        sess: &Session,
        _turn_diff_tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(
                inv.ctx.call_id,
                "request_resources expects function-call arguments",
            );
        };

        handle_request_resources(sess, &inv.ctx, arguments).await
    }
}

async fn handle_request_resources(
    sess: &Session,
    ctx: &ToolCallCtx,
    arguments: String,
) -> ResponseInputItem {
    let args: RequestResourcesArgs = match serde_json::from_str(&arguments) {
        Ok(args) => args,
        Err(error) => {
            return tool_error(
                ctx.call_id.clone(),
                format!("invalid request_resources arguments: {error}"),
            );
        }
    };

    let current = sess.persistent_resource_limits();
    if let Err(error) = validate_resource_request(&current, &args.resources) {
        return tool_error(ctx.call_id.clone(), error);
    }

    if matches!(sess.get_approval_policy(), AskForApproval::Never)
        || matches!(
            sess.get_approval_policy(),
            AskForApproval::Reject(ref config) if config.rejects_request_permissions()
        )
    {
        return tool_output(
            ctx.call_id.clone(),
            serialize_response(RequestResourcesResponse {
                resources: ResourceRequestProfile::default(),
                effective_resources: ResourceRequestProfile::default(),
                clamp_reason: None,
                scope: ResourceGrantScope::NextCommand,
            }),
        );
    }

    let response_rx = match sess.register_pending_request_resources(
        ctx.sub_id.clone(),
        ctx.call_id.clone(),
        args.resources.clone(),
    ) {
        Ok(receiver) => receiver,
        Err(error) => return tool_error(ctx.call_id.clone(), error),
    };

    sess.send_ordered_from_ctx(
        ctx,
        EventMsg::RequestResources(RequestResourcesEvent {
            call_id: ctx.call_id.clone(),
            turn_id: ctx.sub_id.clone(),
            reason: args.reason,
            current,
            requested: args.resources,
        }),
    )
    .await;

    let Ok(response) = response_rx.await else {
        return tool_error(
            ctx.call_id.clone(),
            "request_resources was cancelled before receiving a response",
        );
    };

    tool_output(ctx.call_id.clone(), serialize_response(response))
}

fn validate_resource_request(
    current: &ResourceRequestProfile,
    requested: &ResourceRequestProfile,
) -> Result<(), &'static str> {
    if requested.is_empty() || requested.memory_max_mb == Some(0) || requested.pids_max == Some(0) {
        return Err("request_resources requires a positive memory_max_mb or pids_max");
    }

    if let (Some(requested), Some(current)) = (requested.memory_max_mb, current.memory_max_mb)
        && requested <= current
    {
        return Err("memory_max_mb must be greater than the current Code-managed limit");
    }
    if let (Some(requested), Some(current)) = (requested.pids_max, current.pids_max)
        && requested <= current
    {
        return Err("pids_max must be greater than the current Code-managed limit");
    }

    Ok(())
}

fn serialize_response(response: RequestResourcesResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use code_protocol::request_resources::RequestResourcesArgs;
    use code_protocol::request_resources::ResourceRequestProfile;

    #[test]
    fn request_requires_positive_resource() {
        let args: RequestResourcesArgs =
            serde_json::from_str(r#"{"resources":{"memory_max_mb":2048}}"#).expect("valid request");
        assert_eq!(args.resources.memory_max_mb, Some(2048));
        assert!(!args.resources.is_empty());
    }

    #[test]
    fn request_resources_rejects_any_non_increasing_requested_field() {
        let current = ResourceRequestProfile {
            memory_max_mb: Some(512),
            pids_max: Some(256),
        };
        let requested = ResourceRequestProfile {
            memory_max_mb: Some(1024),
            pids_max: Some(128),
        };

        assert_eq!(
            super::validate_resource_request(&current, &requested),
            Err("pids_max must be greater than the current Code-managed limit")
        );
    }

    #[test]
    fn request_resources_accepts_partial_increase() {
        let current = ResourceRequestProfile {
            memory_max_mb: Some(512),
            pids_max: Some(256),
        };
        let requested = ResourceRequestProfile {
            memory_max_mb: Some(1024),
            pids_max: None,
        };

        assert_eq!(
            super::validate_resource_request(&current, &requested),
            Ok(())
        );
    }

    #[test]
    fn request_resources_rejects_empty_and_zero_profiles() {
        let current = ResourceRequestProfile {
            memory_max_mb: Some(512),
            pids_max: Some(256),
        };

        assert_eq!(
            super::validate_resource_request(&current, &ResourceRequestProfile::default()),
            Err("request_resources requires a positive memory_max_mb or pids_max")
        );
        assert_eq!(
            super::validate_resource_request(
                &current,
                &ResourceRequestProfile {
                    memory_max_mb: Some(0),
                    pids_max: None,
                },
            ),
            Err("request_resources requires a positive memory_max_mb or pids_max")
        );
    }
}
