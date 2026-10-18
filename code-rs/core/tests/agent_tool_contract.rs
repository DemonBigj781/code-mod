mod common;

use code_core::protocol::{EventMsg, InputItem, Op};
use code_core::{CodexAuth, ConversationManager, ModelProviderInfo, built_in_model_providers};
use common::{load_default_config_for_test, load_sse_fixture_with_id, wait_for_event};
use serde_json::{Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn agent_identifier_actions_accept_the_advertised_arguments_without_spawning() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let requests = [
        json!({"action":"status","status":{"agent_id":"missing-agent"}}),
        json!({"action":"result","result":{"agent_id":"missing-agent"}}),
        json!({"action":"wait","wait":{"agent_id":"missing-agent","timeout_seconds":1}}),
        json!({"action":"cancel","cancel":{"agent_id":"missing-agent"}}),
        json!({"action":"list"}),
        json!({"action":"create","create":{"task":"Inspect the requested source files","model":"not-enabled"}}),
    ];
    let mut first = String::new();
    for (index, arguments) in requests.iter().enumerate() {
        let event = json!({"type":"response.output_item.done","item":{
            "type":"function_call","id":format!("agent-call-{index}"),
            "call_id":format!("agent-call-{index}"),"name":"agent","arguments":arguments.to_string()
        }});
        first.push_str(&format!(
            "event: response.output_item.done\ndata: {event}\n\n"
        ));
    }
    first.push_str(&load_sse_fixture_with_id(
        "tests/fixtures/completed_template.json",
        "agent-calls",
    ));
    let second =
        load_sse_fixture_with_id("tests/fixtures/completed_template.json", "agent-results");
    for body in [first, second] {
        Mock::given(method("POST"))
            .and(path_regex(".*/responses$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let mut config = load_default_config_for_test(&home);
    config.cwd = work.path().to_owned();
    config.input_compression.enabled = false;
    config.subagents_enabled = false;
    config.model = "gpt-5.1-codex".into();
    config.model_provider = ModelProviderInfo {
        base_url: Some(format!("{}/v1", server.uri())),
        ..built_in_model_providers(None)["openai"].clone()
    };
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("local-fixture-only"));
    let conversation = manager.new_conversation(config).await.unwrap().conversation;
    conversation
        .submit(Op::UserInput {
            items: vec![InputItem::Text {
                text: "Check tool request contracts".into(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();
    wait_for_event(&conversation, |event| {
        matches!(event, EventMsg::TaskComplete(_))
    })
    .await;
    conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&conversation, |event| {
        matches!(event, EventMsg::ShutdownComplete)
    })
    .await;
    let received = server.received_requests().await.unwrap();
    let followup = received
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/responses"))
        .nth(1)
        .expect("tool outputs must reach the next provider request")
        .body_json::<Value>()
        .unwrap();
    let outputs = followup["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), requests.len());
    for index in 0..4 {
        let output = outputs
            .iter()
            .find(|o| o["call_id"] == format!("agent-call-{index}"))
            .unwrap()["output"]
            .as_str()
            .unwrap();
        assert!(
            !output.contains("requires"),
            "schema-valid lookup rejected: {output}"
        );
        assert!(
            output.contains("not found") || output.contains("Failed to cancel"),
            "unexpected lookup result: {output}"
        );
    }
    let list: Value = serde_json::from_str(
        outputs
            .iter()
            .find(|o| o["call_id"] == "agent-call-4")
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(list["agents"].as_array().unwrap().is_empty());
    let create: Value = serde_json::from_str(
        outputs
            .iter()
            .find(|o| o["call_id"] == "agent-call-5")
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(create["status"], "disabled");
}
