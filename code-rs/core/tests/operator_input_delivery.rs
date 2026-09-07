#![allow(clippy::unwrap_used)]

mod common;

use common::{load_default_config_for_test, load_sse_fixture_with_id, wait_for_event};
use code_core::built_in_model_providers;
use code_core::protocol::{AskForApproval, EventMsg, InputItem, Op, SandboxPolicy};
use code_core::{CodexAuth, ConversationManager, ModelProviderInfo};
use tempfile::TempDir;
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
