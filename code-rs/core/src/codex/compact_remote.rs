use std::sync::Arc;

use super::compact::{
    apply_emergency_compaction_fallback,
    emit_compaction_telemetry,
    is_context_overflow_error,
    perform_compaction,
    prune_orphan_tool_outputs,
    response_input_from_core_items,
    run_inline_auto_compact_task,
    sanitize_items_for_compact,
    send_compaction_checkpoint_warning,
};
use super::Session;
use super::TurnContext;
use crate::Prompt;
use crate::error::CodexErr;
use crate::error::Result as CodexResult;
use crate::error::RetryAfter;
use crate::protocol::AgentMessageEvent;
use crate::protocol::ErrorEvent;
use crate::protocol::EventMsg;
use crate::protocol::InputItem;
use code_protocol::models::ResponseItem;
use code_protocol::protocol::CompactedItem;
use code_protocol::protocol::RolloutItem;
use code_otel::otel_event_manager::{ContextManagementOutcome, ContextManagementPath};
use crate::util::backoff;
use reqwest::StatusCode;
use std::time::Instant;

const MAX_REMOTE_COMPACT_CONTEXT_OVERFLOW_TRIMS: usize = 32;
const MAX_REMOTE_COMPACT_USAGE_LIMIT_RETRIES: usize = 2;

fn should_fallback_to_local_compaction(err: &CodexErr) -> bool {
    match err {
        CodexErr::UnexpectedStatus(response) => {
            response.status.is_server_error()
                || matches!(
                    response.status,
                    StatusCode::BAD_REQUEST
                        | StatusCode::NOT_FOUND
                        | StatusCode::METHOD_NOT_ALLOWED
                        | StatusCode::UNPROCESSABLE_ENTITY
                        | StatusCode::NOT_IMPLEMENTED
                )
        }
        CodexErr::Stream(..)
        | CodexErr::ServerError(_)
        | CodexErr::ServerOverloaded
        | CodexErr::Reqwest(_)
        | CodexErr::Json(_)
        | CodexErr::Io(_) => true,
        CodexErr::RetryLimit(retry) => retry.retryable,
        _ => false,
    }
}

fn remote_compaction_fallback_message(err: &CodexErr) -> String {
    format!(
        "Remote compaction failed; using local summary fallback. Original error: {err}"
    )
}

async fn report_remote_compaction_fallback(sess: &Session, sub_id: &str, err: &CodexErr) {
    let message = remote_compaction_fallback_message(err);
    tracing::warn!(
        error = %err,
        compaction_path = "remote",
        fallback_path = "local_summary",
        "remote compaction failed; using local summary fallback"
    );
    let event = sess.make_event(
        sub_id,
        EventMsg::Error(ErrorEvent {
            message,
        }),
    );
    sess.send_event(event).await;
}

pub(super) async fn run_inline_remote_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    extra_input: Vec<InputItem>,
) -> Vec<ResponseItem> {
    let sub_id = sess.next_internal_sub_id();
    match run_remote_compact_task_inner(&sess, &turn_context, &sub_id, extra_input).await {
        Ok(history) => history,
        Err(err) if should_fallback_to_local_compaction(&err) => {
            report_remote_compaction_fallback(&sess, &sub_id, &err).await;
            run_inline_auto_compact_task(sess, turn_context).await
        }
        Err(err) => {
            let event = sess.make_event(
                &sub_id,
                EventMsg::Error(ErrorEvent {
                    message: format!("remote compact failed: {err}"),
                }),
            );
            sess.send_event(event).await;
            Vec::new()
        }
    }
}

pub(super) async fn run_remote_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    sub_id: String,
    extra_input: Vec<InputItem>,
) -> CodexResult<()> {
    let fallback_input = extra_input.clone();
    match run_remote_compact_task_inner(&sess, &turn_context, &sub_id, extra_input).await {
        Ok(_history) => {
            // Mirror local compaction behaviour: clear the running task when the
            // compaction finished successfully so the UI can unblock.
            sess.revoke_deno_turn_permissions().await;
            sess.remove_task(&sub_id);
            Ok(())
        }
        Err(err) if should_fallback_to_local_compaction(&err) => {
            report_remote_compaction_fallback(&sess, &sub_id, &err).await;
            perform_compaction(sess, turn_context, sub_id, fallback_input, true).await
        }
        Err(err) => {
            let event = sess.make_event(
                &sub_id,
                EventMsg::Error(ErrorEvent {
                    message: err.to_string(),
                }),
            );
            sess.send_event(event).await;
            Err(err)
        }
    }
}

async fn run_remote_compact_task_inner(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    sub_id: &str,
    extra_input: Vec<InputItem>,
) -> CodexResult<Vec<ResponseItem>> {
    let mut turn_items = sess.turn_input_with_history({
        if extra_input.is_empty() {
            Vec::new()
        } else {
            let response_input = response_input_from_core_items(extra_input);
            vec![ResponseItem::from(response_input)]
        }
    });

    turn_items = sanitize_items_for_compact(turn_items);
    let telemetry_started = Instant::now();
    let telemetry_input_count = turn_items.len();
    emit_compaction_telemetry(
        sess,
        ContextManagementPath::RemoteCompaction,
        ContextManagementOutcome::Started,
        None,
        Some(telemetry_input_count),
        None,
        None,
        None,
    );
    let mut truncated_count = 0usize;
    let max_retries = turn_context.client.get_provider().stream_max_retries();
    let mut retries = 0;
    let mut usage_limit_retries = 0usize;
    let new_history = loop {
        prune_orphan_tool_outputs(&mut turn_items);

        let mut prompt = Prompt {
            input: turn_items.clone(),
            base_instructions_override: turn_context.base_instructions.clone(),
            include_additional_instructions: false,
            log_tag: Some("codex/remote-compact".to_owned()),
            ..Prompt::default()
        };

        let _used_fallback_model_metadata = sess.apply_remote_model_overrides(&mut prompt).await;

        match turn_context
            .client
            .compact_conversation_history(&prompt)
            .await
        {
            Ok(history) => {
                if truncated_count > 0 {
                    tracing::warn!(
                        "Context window exceeded during remote compact; trimmed {truncated_count} item(s) from prompt"
                    );
                }
                break history;
            }
            Err(err) if is_context_overflow_error(&err) => {
                if turn_items.len() > 1
                    && truncated_count < MAX_REMOTE_COMPACT_CONTEXT_OVERFLOW_TRIMS
                {
                    tracing::warn!(
                        "Context window exceeded while remote compacting; dropping oldest item ({} remaining)",
                        turn_items.len().saturating_sub(1)
                    );
                    turn_items.remove(0);
                    truncated_count = truncated_count.saturating_add(1);
                    retries = 0;
                    usage_limit_retries = 0;
                    continue;
                }

                if truncated_count >= MAX_REMOTE_COMPACT_CONTEXT_OVERFLOW_TRIMS {
                    let reason = format!(
                        "Remote compact trimmed {truncated_count} items but still exceeded the context window."
                    );
                    emit_compaction_telemetry(
                        sess,
                        ContextManagementPath::RemoteCompaction,
                        ContextManagementOutcome::Failed,
                        Some(telemetry_started),
                        Some(telemetry_input_count),
                        None,
                        Some(truncated_count),
                        Some(reason.clone()),
                    );
                    return Ok(
                        apply_emergency_compaction_fallback(
                            sess,
                            turn_context.as_ref(),
                            sub_id,
                            &reason,
                        )
                        .await,
                    );
                }

                let reason = "Remote compact failed: context overflow even with minimal input.";
                emit_compaction_telemetry(
                    sess,
                    ContextManagementPath::RemoteCompaction,
                    ContextManagementOutcome::Failed,
                    Some(telemetry_started),
                    Some(telemetry_input_count),
                    None,
                    Some(truncated_count),
                    Some(reason.to_owned()),
                );
                return Ok(
                    apply_emergency_compaction_fallback(
                        sess,
                        turn_context.as_ref(),
                        sub_id,
                        reason,
                    )
                    .await,
                );
            }
            Err(CodexErr::UsageLimitReached(limit_err)) => {
                if usage_limit_retries >= MAX_REMOTE_COMPACT_USAGE_LIMIT_RETRIES {
                    let reason = "Remote compact hit persistent usage limits and cannot continue.";
                    emit_compaction_telemetry(
                        sess,
                        ContextManagementPath::RemoteCompaction,
                        ContextManagementOutcome::Failed,
                        Some(telemetry_started),
                        Some(telemetry_input_count),
                        None,
                        Some(truncated_count),
                        Some(reason.to_owned()),
                    );
                    return Ok(
                        apply_emergency_compaction_fallback(
                            sess,
                            turn_context.as_ref(),
                            sub_id,
                            reason,
                        )
                        .await,
                    );
                }
                usage_limit_retries = usage_limit_retries.saturating_add(1);
                let now = chrono::Utc::now();
                let retry_after = limit_err
                    .retry_after(now)
                    .unwrap_or_else(|| RetryAfter::from_duration(RetryAfter::DEFAULT_DELAY, now));
                let mut message = format!("{limit_err} Auto-retrying");
                message.push('…');
                sess.notify_stream_error(sub_id, message).await;
                tokio::time::sleep(retry_after.delay).await;
                retries = 0;
            }
            Err(err) if should_fallback_to_local_compaction(&err) => {
                emit_compaction_telemetry(
                    sess,
                    ContextManagementPath::RemoteCompaction,
                    ContextManagementOutcome::Fallback,
                    Some(telemetry_started),
                    Some(telemetry_input_count),
                    None,
                    Some(truncated_count),
                    Some(err.to_string()),
                );
                return Err(err);
            }
            Err(err) => {
                if retries < max_retries {
                    retries += 1;
                    let delay = backoff(retries);
                    sess
                        .notify_stream_error(
                            sub_id,
                            format!(
                                "remote compact error: {err}; retrying {retries}/{max_retries} in {delay:?}…"
                            ),
                        )
                        .await;
                    tokio::time::sleep(delay).await;
                    continue;
                }

                emit_compaction_telemetry(
                    sess,
                    ContextManagementPath::RemoteCompaction,
                    ContextManagementOutcome::Failed,
                    Some(telemetry_started),
                    Some(telemetry_input_count),
                    None,
                    Some(truncated_count),
                    Some(err.to_string()),
                );
                return Err(err);
            }
        }
    };

    sess.replace_history(new_history.clone());
    {
        let mut state = crate::codex::lock_or_panic!(sess.state);
        state.token_usage_info = None;
    }

    send_compaction_checkpoint_warning(sess, sub_id).await;

    let rollout_item = RolloutItem::Compacted(CompactedItem {
        message: "Conversation history compacted.".to_owned(),
        replacement_history: None,
    });
    sess.persist_rollout_items(&[rollout_item]).await;

    let event = sess.make_event(
        sub_id,
        EventMsg::AgentMessage(AgentMessageEvent {
            message: "Compact task completed".to_owned(),
        }),
    );
    sess.send_event(event).await;
    emit_compaction_telemetry(
        sess,
        ContextManagementPath::RemoteCompaction,
        ContextManagementOutcome::Completed,
        Some(telemetry_started),
        Some(telemetry_input_count),
        Some(new_history.len()),
        Some(truncated_count),
        None,
    );

    Ok(new_history)
}

#[cfg(test)]
mod tests {
    use super::remote_compaction_fallback_message;
    use super::should_fallback_to_local_compaction;
    use crate::error::CodexErr;
    use crate::error::UnexpectedResponseError;
    use crate::error::UsageLimitReachedError;
    use reqwest::StatusCode;

    #[test]
    fn remote_service_failures_fall_back_locally_but_account_failures_do_not() {
        let not_found = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::NOT_FOUND,
            body: r#"{"detail":"Not Found"}"#.to_string(),
            request_id: None,
        });
        let server_error = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: "temporary failure".to_string(),
            request_id: None,
        });
        let rate_limited = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: "rate limited".to_string(),
            request_id: None,
        });
        let unauthorized = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::UNAUTHORIZED,
            body: "expired token".to_string(),
            request_id: None,
        });
        let forbidden = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::FORBIDDEN,
            body: "account cannot compact".to_string(),
            request_id: None,
        });
        let stream_error = CodexErr::Stream("connection reset".to_owned(), None, None);
        let usage_limit = CodexErr::UsageLimitReached(UsageLimitReachedError {
            plan_type: None,
            resets_in_seconds: None,
        });
        let expired_auth = CodexErr::AuthRefreshPermanent("expired".to_owned());

        assert!(should_fallback_to_local_compaction(&not_found));
        assert!(should_fallback_to_local_compaction(&server_error));
        assert!(should_fallback_to_local_compaction(&stream_error));
        assert!(!should_fallback_to_local_compaction(&rate_limited));
        assert!(!should_fallback_to_local_compaction(&unauthorized));
        assert!(!should_fallback_to_local_compaction(&forbidden));
        assert!(!should_fallback_to_local_compaction(&usage_limit));
        assert!(!should_fallback_to_local_compaction(&expired_auth));
        assert!(!should_fallback_to_local_compaction(&CodexErr::Interrupted));
    }

    #[test]
    fn local_fallback_message_preserves_the_exact_remote_error() {
        let error = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: StatusCode::BAD_REQUEST,
            body: r#"{"error":"Input required: prompt or messages"}"#.to_string(),
            request_id: Some("compact-request-7".to_string()),
        });

        assert_eq!(
            remote_compaction_fallback_message(&error),
            "Remote compaction failed; using local summary fallback. Original error: unexpected status 400 Bad Request: {\"error\":\"Input required: prompt or messages\"}, request id: compact-request-7",
        );
    }
}
