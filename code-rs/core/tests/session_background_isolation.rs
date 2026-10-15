#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use code_core::protocol::{AskForApproval, EventMsg, InputItem, Op, SandboxPolicy};
use code_core::{CodexAuth, ConversationManager, built_in_model_providers};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::time::{Duration, timeout};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn response(item: Value, id: usize) -> ResponseTemplate {
    let item = json!({"type": "response.output_item.done", "item": item});
    let completed = json!({
        "type": "response.completed",
        "response": {"id": format!("response-{id}"), "usage": {
            "input_tokens": 0, "output_tokens": 0, "total_tokens": 0,
            "input_tokens_details": null, "output_tokens_details": null
        }}
    });
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(format!(
            "event: response.output_item.done\ndata: {item}\n\nevent: response.completed\ndata: {completed}\n\n"
        ))
}

#[tokio::test]
async fn background_completions_stay_in_their_own_conversations() {
    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let server = MockServer::start().await;
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("test-key"));
    let mut conversations = Vec::new();

    for name in ["alpha", "bravo"] {
        let request_index = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path(format!("/{name}/responses")))
            .respond_with(move |_: &wiremock::Request| {
                let index = request_index.fetch_add(1, Ordering::SeqCst);
                let item = if index < 2 {
                    // The first job backgrounds after 10s. The second keeps the
                    // turn alive until both sessions' first jobs have finished.
                    let script = if index == 0 {
                        format!("sleep 11; printf 'completion-{name}'")
                    } else {
                        "sleep 4".to_owned()
                    };
                    json!({
                        "type": "function_call", "id": format!("{name}-{index}"),
                        "call_id": format!("{name}-{index}"), "name": "shell",
                        "arguments": json!({"command": ["sh", "-c", script]}).to_string()
                    })
                } else {
                    json!({"type": "message", "id": format!("{name}-done"),
                        "role": "assistant", "content": [{"type": "output_text", "text": "done"}]})
                };
                response(item, index)
            })
            .mount(&server)
            .await;

        let mut config = common::load_default_config_for_test(&home);
        config.cwd = workspace.path().to_path_buf();
        config.approval_policy = AskForApproval::Never;
        config.sandbox_policy = SandboxPolicy::DangerFullAccess;
        config.model = "gpt-5.1-codex".to_owned();
        config.model_provider = built_in_model_providers(None)["openai"].clone();
        config.model_provider.base_url = Some(format!("{}/{name}", server.uri()));
        conversations.push(manager.new_conversation(config).await.unwrap().conversation);
    }

    for conversation in &conversations {
        conversation
            .submit(Op::UserInput {
                items: vec![InputItem::Text {
                    text: "Run the isolation test.".to_owned(),
                }],
                final_output_json_schema: None,
            })
            .await
            .unwrap();
    }
    for conversation in &conversations {
        timeout(Duration::from_secs(60), async {
            loop {
                if matches!(
                    conversation.next_event().await.unwrap().msg,
                    EventMsg::TaskComplete(_)
                ) {
                    break;
                }
            }
        })
        .await
        .expect("conversation must complete");
        conversation.submit(Op::Shutdown).await.unwrap();
    }

    let requests = server.received_requests().await.unwrap();
    for (own, other) in [("alpha", "bravo"), ("bravo", "alpha")] {
        let request = requests
            .iter()
            .rev()
            .find(|request| request.url.path() == format!("/{own}/responses"))
            .expect("conversation made a model request");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let completions: Vec<_> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"] == "developer")
            .flat_map(|item| item["content"].as_array().into_iter().flatten())
            .filter_map(|content| content["text"].as_str())
            .filter(|text| text.starts_with("Background shell completed"))
            .collect();
        assert!(
            completions
                .iter()
                .any(|text| text.contains(&format!("completion-{own}"))),
            "{own} did not receive its own completion: {completions:?}"
        );
        assert!(
            !completions
                .iter()
                .any(|text| text.contains(&format!("completion-{other}"))),
            "{own} received the other tab's completion: {completions:?}"
        );
    }
}

#[tokio::test]
async fn ai_titles_follow_topic_changes_and_stay_session_local() {
    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let server = MockServer::start().await;
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("test-key"));
    let mut sessions = Vec::new();
    for name in ["alpha", "bravo"] {
        let index = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST")).and(path(format!("/{name}/responses")))
            .respond_with(move |_: &wiremock::Request| {
                let index = index.fetch_add(1, Ordering::SeqCst);
                let item = if index % 2 == 0 {
                    json!({"type": "function_call", "id": format!("title-{index}"),
                        "call_id": format!("title-{index}"), "name": "set_session_title",
                        "arguments": json!({"title": format!("{name} topic {}", index / 2)}).to_string()})
                } else {
                    json!({"type": "message", "id": format!("done-{index}"), "role": "assistant",
                        "content": [{"type": "output_text", "text": "done"}]})
                };
                response(item, index)
            }).mount(&server).await;
        let mut config = common::load_default_config_for_test(&home);
        config.cwd = workspace.path().to_path_buf();
        config.approval_policy = AskForApproval::Never;
        config.sandbox_policy = SandboxPolicy::DangerFullAccess;
        config.model = "gpt-5.1-codex".into();
        config.model_provider = built_in_model_providers(None)["openai"].clone();
        config.model_provider.base_url = Some(format!("{}/{name}", server.uri()));
        sessions.push((name, manager.new_conversation(config).await.unwrap()));
    }
    for topic in 0..2 {
        for (_, session) in &sessions {
            session
                .conversation
                .submit(Op::UserInput {
                    items: vec![InputItem::Text {
                        text: format!("Discuss topic {topic}."),
                    }],
                    final_output_json_schema: None,
                })
                .await
                .unwrap();
        }
        for (name, session) in &sessions {
            let expected = format!("{name} topic {topic}");
            timeout(Duration::from_secs(30), async {
                let mut saw_title = false;
                loop {
                    match session.conversation.next_event().await.unwrap().msg {
                        EventMsg::ThreadNameUpdated(update) => {
                            assert_eq!(update.thread_id.to_string(), session.conversation_id.to_string());
                            assert_eq!(update.thread_name.as_deref(), Some(expected.as_str()));
                            saw_title = true;
                        }
                        EventMsg::TaskComplete(_) => {
                            assert!(saw_title);
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();
        }
        let catalog = code_core::SessionCatalog::new(home.path().to_path_buf());
        for (name, session) in &sessions {
            let entry = catalog
                .find_by_id(&session.conversation_id.to_string())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(entry.nickname, Some(format!("{name} topic {topic}")));
        }
    }
    for (_, session) in &sessions {
        session.conversation.submit(Op::Shutdown).await.unwrap();
    }
}
