#![allow(clippy::unwrap_used)]

mod common;

use common::{load_default_config_for_test, load_sse_fixture_with_id, wait_for_event};
use code_core::built_in_model_providers;
use code_core::protocol::{AskForApproval, EventMsg, InputItem, Op, ReviewRequest, SandboxPolicy};
use code_core::{CodexAuth, ConversationManager, ModelProviderInfo};
use code_protocol::protocol::ReviewTarget;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn input_texts(body: &serde_json::Value) -> Vec<&str> {
    body["input"]
        .as_array()
        .expect("responses request should include input")
        .iter()
        .filter(|item| item.get("role").and_then(|value| value.as_str()) == Some("user"))
        .filter_map(|item| item.get("content").and_then(|value| value.as_array()))
        .flatten()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("input_text"))
        .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
        .collect()
}

fn input_items<'a>(body: &'a serde_json::Value, item_type: &str) -> Vec<&'a serde_json::Value> {
    body["input"]
        .as_array()
        .expect("responses request should include input")
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some(item_type))
        .collect()
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn wait_for_response_requests(server: &MockServer, count: usize) {
    timeout(std::time::Duration::from_secs(3), async {
        loop {
            if server.received_requests().await.unwrap().iter()
                .filter(|request| request.method.as_str() == "POST" && request.url.path().ends_with("/responses"))
                .count() >= count
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("provider request should arrive promptly");
}

async fn check_operator_input_preempts_blocked_request(retry: bool) {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;
    if retry {
        Mock::given(method("POST"))
            .and(path_regex(".*/responses$"))
            .respond_with(sse_response(String::new()))
            .up_to_n_times(1)
            .mount(&server).await;
    }
    let completed = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "fresh");
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(sse_response(completed.clone()).set_delay(std::time::Duration::from_secs(30)))
        .up_to_n_times(1)
        .mount(&server).await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(sse_response(completed))
        .mount(&server).await;

    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.input_compression.enabled = false;
    config.model = "gpt-5.1-codex".to_owned();
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config).await.unwrap().conversation;
    conversation.submit(Op::UserInput {
        items: vec![InputItem::Text { text: "original request".into() }],
        final_output_json_schema: None,
    }).await.unwrap();
    let blocked_request_count = if retry { 2 } else { 1 };
    wait_for_response_requests(&server, blocked_request_count).await;
    conversation.submit(Op::QueueUserInput {
        items: vec![InputItem::Text { text: "operator correction must take priority".into() }],
    }).await.unwrap();

    let delivered = timeout(std::time::Duration::from_secs(3), async {
        loop {
            let requests = server.received_requests().await.unwrap();
            if let Some(body) = requests.iter()
                .filter(|request| request.method.as_str() == "POST" && request.url.path().ends_with("/responses"))
                .skip(blocked_request_count)
                .map(|request| request.body_json::<serde_json::Value>().unwrap())
                .find(|body| input_texts(body).contains(&"operator correction must take priority"))
            {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;
    let body = delivered.expect("new input must preempt the blocked request, not wait 30 seconds");
    let texts = input_texts(&body);
    assert_eq!(texts.iter().filter(|text| **text == "original request").count(), 1);
    assert_eq!(texts.iter().filter(|text| **text == "operator correction must take priority").count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn operator_input_preempts_blocked_provider_request() {
    check_operator_input_preempts_blocked_request(false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn operator_input_preempts_blocked_provider_retry() {
    check_operator_input_preempts_blocked_request(true).await;
}

fn text_files_under(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(text_files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn json_contains_string(value: &serde_json::Value, expected: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value == expected,
        serde_json::Value::Array(values) => {
            values.iter().any(|value| json_contains_string(value, expected))
        }
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| json_contains_string(value, expected)),
        _ => false,
    }
}

fn count_items_with_type_and_call_id(
    value: &serde_json::Value,
    item_type: &str,
    call_id: &str,
) -> usize {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| count_items_with_type_and_call_id(value, item_type, call_id))
            .sum(),
        serde_json::Value::Object(values) => {
            let matches_item = values.get("type").and_then(|value| value.as_str()) == Some(item_type)
                && values.get("call_id").and_then(|value| value.as_str()) == Some(call_id);
            usize::from(matches_item)
                + values
                    .values()
                    .map(|value| count_items_with_type_and_call_id(value, item_type, call_id))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn one_operator_submission_reaches_the_model_once_in_compressed_form() {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;
    let sse = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "resp-1");

    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.approval_policy = AskForApproval::Never;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    config.model = "gpt-5.1-codex".to_owned();

    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation;
    let original = "Please update the documentation.\n\nPlease update the documentation.";

    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text { text: original.into() }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::TaskComplete(_))).await;

    let requests = server.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|request| request.url.path().ends_with("/responses"))
        .expect("model request");
    let body: serde_json::Value = request.body_json().unwrap();
    let texts = input_texts(&body);
    assert_eq!(
        texts.iter().filter(|text| **text == "Update the documentation.").count(),
        1,
    );
    assert!(!texts.contains(&original));

    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;
    let persisted_original = text_files_under(code_home.path())
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|contents| {
            contents
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .any(|value| json_contains_string(&value, original));
    assert!(persisted_original, "local history should retain the original operator text");
}

#[cfg(not(windows))]
#[tokio::test(flavor = "current_thread")]
async fn turn_complete_notification_contains_only_the_current_submission() {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;
    let sse = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "notify");

    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(sse_response(sse))
        .mount(&server)
        .await;

    let notifications_path = code_home.path().join("notifications.jsonl");
    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.approval_policy = AskForApproval::Never;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;
    config.input_compression.enabled = false;
    config.notify = Some(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        r#"printf '%s\n' "$2" >> "$1""#.to_owned(),
        "code-notify".to_owned(),
        notifications_path.display().to_string(),
    ]);
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    config.model = "gpt-5.1-codex".to_owned();

    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation;

    for text in ["first submission", "second submission"] {
        conversation
            .submit(Op::UserInput {
                items: vec![InputItem::Text { text: text.into() }],
                final_output_json_schema: None,
            })
            .await
            .unwrap();
        wait_for_event(&conversation, |event| matches!(event, EventMsg::TaskComplete(_))).await;
    }

    let notifications = timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(contents) = std::fs::read_to_string(&notifications_path) {
                let notifications = contents
                    .lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .collect::<Vec<_>>();
                if notifications.len() >= 2 {
                    return notifications;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("notifier should receive both completed turns");

    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;

    assert_eq!(
        notifications[0]["input-messages"],
        json!(["first submission"])
    );
    assert_eq!(
        notifications[1]["input-messages"],
        json!(["second submission"]),
        "notification payload must not grow with retained conversation history"
    );
}

#[cfg(not(windows))]
#[tokio::test(flavor = "current_thread")]
async fn queued_operator_input_and_tool_output_reach_each_model_request_once() {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;

    let function_call_args = json!({
        "command": ["sh", "-c", "sleep 0.5; printf tool-finished"],
        "workdir": project_dir.path(),
        "timeout_ms": null,
        "sandbox_permissions": null,
        "justification": null,
    });
    let function_call_item = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "call-1",
            "call_id": "call-1",
            "name": "shell",
            "arguments": function_call_args.to_string(),
        }
    });
    let completed_one = json!({
        "type": "response.completed",
        "response": {
            "id": "resp-1",
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0
            }
        }
    });
    let body_one = format!(
        "event: response.output_item.done\ndata: {function_call_item}\n\n\
event: response.completed\ndata: {completed_one}\n\n"
    );

    let message_item = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "id": "msg-1",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "first turn done"}],
        }
    });
    let completed_two = json!({
        "type": "response.completed",
        "response": {
            "id": "resp-2",
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0
            }
        }
    });
    let body_two = format!(
        "event: response.output_item.done\ndata: {message_item}\n\n\
event: response.completed\ndata: {completed_two}\n\n"
    );

    let message_item_three = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "id": "msg-2",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "second turn done"}],
        }
    });
    let completed_three = json!({
        "type": "response.completed",
        "response": {
            "id": "resp-3",
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0
            }
        }
    });
    let body_three = format!(
        "event: response.output_item.done\ndata: {message_item_three}\n\n\
event: response.completed\ndata: {completed_three}\n\n"
    );

    for body in [body_one, body_two, body_three] {
        Mock::given(method("POST"))
            .and(path_regex(".*/responses$"))
            .respond_with(sse_response(body))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }

    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.approval_policy = AskForApproval::Never;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;
    config.input_compression.enabled = false;
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    config.model = "gpt-5.1-codex".to_owned();

    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation;

    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "run the tool".into(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    loop {
        let event = timeout(std::time::Duration::from_secs(5), conversation.next_event())
            .await
            .expect("timeout waiting for exec begin")
            .expect("event stream should remain open");
        if let EventMsg::ExecCommandBegin(event) = event.msg
            && event.call_id == "call-1"
        {
            break;
        }
    }

    conversation
        .submit(Op::QueueUserInput {
            items: vec![InputItem::Text {
                text: "interrupt at the next boundary".into(),
            }],
        })
        .await
        .unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::TaskComplete(_))).await;

    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "verify retained history".into(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::TaskComplete(_))).await;

    let requests = server.received_requests().await.unwrap();
    let responses_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(responses_requests.len(), 3, "expected exactly three model requests");

    let observed_counts = responses_requests[1..]
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let body: serde_json::Value = request.body_json().unwrap();
            let texts = input_texts(&body);
            let queued_input_count = texts
                .iter()
                .filter(|text| **text == "interrupt at the next boundary")
                .count();
            let tool_output_count = input_items(&body, "function_call_output")
                .iter()
                .filter(|item| item.get("call_id").and_then(|value| value.as_str()) == Some("call-1"))
                .count();
            (index + 2, queued_input_count, tool_output_count)
        })
        .collect::<Vec<_>>();
    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;

    let persisted_tool_output_count = text_files_under(code_home.path())
        .into_iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("jsonl"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|contents| {
            contents
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .map(|value| count_items_with_type_and_call_id(&value, "function_call_output", "call-1"))
        .sum::<usize>();

    assert_eq!(
        (observed_counts, persisted_tool_output_count),
        (vec![(2, 1, 1), (3, 1, 1)], 1),
        "requests and rollout must each retain exactly one copy of queued input and tool output"
    );
}

#[cfg(not(windows))]
#[tokio::test(flavor = "current_thread")]
async fn review_mode_does_not_send_or_consume_queued_operator_input() {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;

    let function_call_args = json!({
        "command": ["sh", "-c", "sleep 1; printf tool-finished"],
        "workdir": project_dir.path(),
        "timeout_ms": null,
        "sandbox_permissions": null,
        "justification": null,
    });
    let function_call_item = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "call-review-boundary",
            "call_id": "call-review-boundary",
            "name": "shell",
            "arguments": function_call_args.to_string(),
        }
    });
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "resp-before-review",
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0
            }
        }
    });
    let first_body = format!(
        "event: response.output_item.done\ndata: {function_call_item}\n\n\
event: response.completed\ndata: {completed}\n\n"
    );
    let review_body = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "review-resp");
    let queued_body = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "queued-resp");

    for body in [first_body, review_body, queued_body] {
        Mock::given(method("POST"))
            .and(path_regex(".*/responses$"))
            .respond_with(sse_response(body))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }

    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.approval_policy = AskForApproval::Never;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;
    config.input_compression.enabled = false;
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    config.model = "gpt-5.1-codex".to_owned();
    config.review_model = config.model.clone();

    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation;

    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "start work before review".into(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    loop {
        let event = timeout(std::time::Duration::from_secs(5), conversation.next_event())
            .await
            .expect("timeout waiting for exec begin")
            .expect("event stream should remain open");
        if let EventMsg::ExecCommandBegin(event) = event.msg
            && event.call_id == "call-review-boundary"
        {
            break;
        }
    }

    conversation
        .submit(Op::QueueUserInput {
            items: vec![InputItem::Text {
                text: "deliver after review".into(),
            }],
        })
        .await
        .unwrap();
    conversation
        .submit(Op::Review {
            review_request: ReviewRequest {
                target: ReviewTarget::Custom {
                    instructions: "review without consuming queued input".to_owned(),
                },
                user_facing_hint: None,
                prompt: "review without consuming queued input".to_owned(),
            },
        })
        .await
        .unwrap();

    timeout(std::time::Duration::from_secs(10), async {
        loop {
            let request_count = server
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|request| request.url.path().ends_with("/responses"))
                .count();
            if request_count >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("review and retained queued turn should both reach the model");

    let requests = server.received_requests().await.unwrap();
    let responses_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(responses_requests.len(), 3, "expected initial, review, and queued requests");

    let review_request: serde_json::Value = responses_requests[1].body_json().unwrap();
    let queued_request: serde_json::Value = responses_requests[2].body_json().unwrap();
    let review_queued_count = input_texts(&review_request)
        .iter()
        .filter(|text| **text == "deliver after review")
        .count();
    let later_queued_count = input_texts(&queued_request)
        .iter()
        .filter(|text| **text == "deliver after review")
        .count();

    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;

    assert_eq!(
        (review_queued_count, later_queued_count),
        (0, 1),
        "review requests must not see queued input, which must remain for the next normal turn"
    );
}

#[cfg(not(windows))]
#[tokio::test(flavor = "current_thread")]
async fn queued_operator_input_reaches_the_first_request_after_a_multi_tool_response() {
    let code_home = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    let server = MockServer::start().await;

    let first_call_args = json!({
        "command": ["sh", "-c", "sleep 0.5; printf first-finished"],
        "workdir": project_dir.path(),
        "timeout_ms": null,
        "sandbox_permissions": null,
        "justification": null,
    });
    let second_call_args = json!({
        "command": ["sh", "-c", "printf second-finished"],
        "workdir": project_dir.path(),
        "timeout_ms": null,
        "sandbox_permissions": null,
        "justification": null,
    });
    let first_call = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "multi-call-1",
            "call_id": "multi-call-1",
            "name": "shell",
            "arguments": first_call_args.to_string(),
        }
    });
    let second_call = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "multi-call-2",
            "call_id": "multi-call-2",
            "name": "shell",
            "arguments": second_call_args.to_string(),
        }
    });
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "multi-tool-resp",
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0
            }
        }
    });
    let first_body = format!(
        "event: response.output_item.done\ndata: {first_call}\n\n\
event: response.output_item.done\ndata: {second_call}\n\n\
event: response.completed\ndata: {completed}\n\n"
    );
    let second_body = load_sse_fixture_with_id("tests/fixtures/completed_template.json", "after-tools");

    for body in [first_body, second_body] {
        Mock::given(method("POST"))
            .and(path_regex(".*/responses$"))
            .respond_with(sse_response(body))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }

    let mut config = load_default_config_for_test(&code_home);
    config.cwd = project_dir.path().to_path_buf();
    config.approval_policy = AskForApproval::Never;
    config.sandbox_policy = SandboxPolicy::DangerFullAccess;
    config.input_compression.enabled = false;
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    config.model = "gpt-5.1-codex".to_owned();

    let conversation = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"))
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation;

    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "run both tools".into(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    loop {
        let event = timeout(std::time::Duration::from_secs(5), conversation.next_event())
            .await
            .expect("timeout waiting for first tool")
            .expect("event stream should remain open");
        if let EventMsg::ExecCommandBegin(event) = event.msg
            && event.call_id == "multi-call-1"
        {
            break;
        }
    }

    conversation
        .submit(Op::QueueUserInput {
            items: vec![InputItem::Text {
                text: "interrupt the multi-tool response".into(),
            }],
        })
        .await
        .unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::TaskComplete(_))).await;

    let requests = server.received_requests().await.unwrap();
    let responses_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(responses_requests.len(), 2, "expected the initial and first safe follow-up request");

    let follow_up: serde_json::Value = responses_requests[1].body_json().unwrap();
    let queued_count = input_texts(&follow_up)
        .iter()
        .filter(|text| **text == "interrupt the multi-tool response")
        .count();
    let first_output_count = input_items(&follow_up, "function_call_output")
        .iter()
        .filter(|item| item.get("call_id").and_then(|value| value.as_str()) == Some("multi-call-1"))
        .count();
    let second_output_count = input_items(&follow_up, "function_call_output")
        .iter()
        .filter(|item| item.get("call_id").and_then(|value| value.as_str()) == Some("multi-call-2"))
        .count();

    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| matches!(event, EventMsg::ShutdownComplete)).await;

    let persisted_counts = text_files_under(code_home.path())
        .into_iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("jsonl"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|contents| {
            contents
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .fold((0, 0), |(first, second), value| {
            (
                first
                    + count_items_with_type_and_call_id(
                        &value,
                        "function_call_output",
                        "multi-call-1",
                    ),
                second
                    + count_items_with_type_and_call_id(
                        &value,
                        "function_call_output",
                        "multi-call-2",
                    ),
            )
        });

    assert_eq!(
        (
            queued_count,
            first_output_count,
            second_output_count,
            persisted_counts,
        ),
        (1, 1, 1, (1, 1)),
        "the first safe follow-up request and rollout must own each queued/tool item once"
    );
}
