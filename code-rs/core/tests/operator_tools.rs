mod common;

use code_core::config::Config;
use code_core::protocol::{AskForApproval, EventMsg, Op, ReviewDecision, SandboxPolicy};
use code_core::{
    CodexAuth, CodexConversation, ConversationManager, ModelProviderInfo, built_in_model_providers,
};
use code_protocol::dynamic_tools::{DynamicToolResponse, DynamicToolSpec};
use common::{load_default_config_for_test, wait_for_event};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use wiremock::MockServer;

struct Fixture {
    _home: TempDir,
    work: TempDir,
    server: MockServer,
    conversation: Arc<CodexConversation>,
}

impl Fixture {
    async fn new(configure: impl FnOnce(&mut Config)) -> Self {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let mut config = load_default_config_for_test(&home);
        config.cwd = work.path().to_owned();
        config.subagents_enabled = false;
        config.input_compression.enabled = false;
        config.approval_policy = AskForApproval::Never;
        config.sandbox_policy = SandboxPolicy::DangerFullAccess;
        config.model = "gpt-5.1-codex".into();
        config.model_provider = ModelProviderInfo {
            base_url: Some(format!("{}/v1", server.uri())),
            ..built_in_model_providers(None)["openai"].clone()
        };
        configure(&mut config);
        let conversation =
            ConversationManager::with_auth(CodexAuth::from_api_key("local-fixture-only"))
                .new_conversation(config)
                .await
                .unwrap()
                .conversation;
        Self {
            _home: home,
            work,
            server,
            conversation,
        }
    }

    async fn notice(&self) -> (String, Option<bool>) {
        let event = wait_for_event(&self.conversation, |e| {
            matches!(e, EventMsg::OperatorToolResponse { .. })
        })
        .await;
        match event {
            EventMsg::OperatorToolResponse { message, success } => (message, success),
            _ => unreachable!(),
        }
    }

    async fn invoke(&self, name: &str, arguments: Option<Value>) -> (String, Option<bool>) {
        self.conversation
            .submit(Op::InvokeTool {
                name: name.into(),
                arguments,
            })
            .await
            .unwrap();
        let result = self.notice().await;
        wait_for_event(&self.conversation, |e| {
            matches!(e, EventMsg::TaskComplete(_))
        })
        .await;
        result
    }

    async fn finish(self) {
        self.conversation.submit(Op::Shutdown).await.unwrap();
        wait_for_event(&self.conversation, |e| {
            matches!(e, EventMsg::ShutdownComplete)
        })
        .await;
        assert!(
            self.server.received_requests().await.unwrap().is_empty(),
            "operator calls must never contact the model provider"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_list_and_native_use_never_request_a_model() {
    let fixture = Fixture::new(|_| {}).await;
    fixture.conversation.submit(Op::ListTools).await.unwrap();
    let (inventory, success) = fixture.notice().await;
    assert_eq!(success, Some(true));
    assert!(inventory.contains("list_dir — native — JSON arguments"));
    assert!(inventory.contains("MCP servers"));
    let (output, success) = fixture.invoke("list_mcp_resources", None).await;
    assert_ne!(success, Some(false), "{output}");
    std::fs::write(fixture.work.path().join("operator-marker.txt"), "fixture").unwrap();
    let (output, success) = fixture
        .invoke("list_dir", Some(json!({"dir_path": fixture.work.path()})))
        .await;
    assert_ne!(success, Some(false), "{output}");
    assert!(output.contains("operator-marker.txt"), "{output}");
    for (name, args, expected) in [
        ("not_loaded", None, "not loaded"),
        ("list_dir", None, "Missing required arguments"),
        ("list_dir", Some(json!("invalid")), "JSON object"),
        (
            "container.exec",
            Some(json!({"command":["true"]})),
            "not enabled",
        ),
    ] {
        let (output, success) = fixture.invoke(name, args).await;
        assert_eq!(success, Some(false), "{output}");
        assert!(output.contains(expected), "{output}");
    }
    let (output, _) = fixture.invoke("agent", Some(json!({"action":"create","create":{"task":"Inspect these fixture files","model":"not-enabled"}}))).await;
    assert!(output.contains("disabled"), "{output}");
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_busy_cancellation_and_client_replies_remain_responsive() {
    let fixture = Fixture::new(|config| {
        config.dynamic_tools.push(DynamicToolSpec {
            namespace: None,
            name: "fixture_client".into(),
            description: "Fixture".into(),
            input_schema: json!({"type":"object","properties":{}}),
            defer_loading: false,
        });
    })
    .await;
    for cancel in [true, false] {
        fixture
            .conversation
            .submit(Op::InvokeTool {
                name: "fixture_client".into(),
                arguments: None,
            })
            .await
            .unwrap();
        let event = wait_for_event(&fixture.conversation, |e| {
            matches!(e, EventMsg::DynamicToolCallRequest(_))
        })
        .await;
        let EventMsg::DynamicToolCallRequest(request) = event else {
            unreachable!()
        };
        fixture
            .conversation
            .submit(Op::InvokeTool {
                name: "list_mcp_resources".into(),
                arguments: None,
            })
            .await
            .unwrap();
        let (message, success) = fixture.notice().await;
        assert_eq!(success, Some(false));
        assert!(message.contains("Busy"));
        fixture.conversation.submit(Op::ListTools).await.unwrap();
        assert!(fixture.notice().await.0.contains("fixture_client — client"));
        if cancel {
            fixture.conversation.submit(Op::Interrupt).await.unwrap();
            wait_for_event(&fixture.conversation, |e| {
                matches!(e, EventMsg::TurnAborted(_))
            })
            .await;
        } else {
            fixture
                .conversation
                .submit(Op::DynamicToolResponse {
                    id: request.call_id,
                    response: DynamicToolResponse {
                        content_items: vec![],
                        success: true,
                    },
                })
                .await
                .unwrap();
            assert_eq!(fixture.notice().await.1, Some(true));
            wait_for_event(&fixture.conversation, |e| {
                matches!(e, EventMsg::TaskComplete(_))
            })
            .await;
        }
    }
    assert_ne!(
        fixture.invoke("list_mcp_resources", None).await.1,
        Some(false)
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_preserve_shell_approval_and_denial() {
    let fixture = Fixture::new(|config| {
        config.approval_policy = AskForApproval::UnlessTrusted;
    })
    .await;
    let marker = fixture.work.path().join("must-not-exist");
    fixture
        .conversation
        .submit(Op::InvokeTool {
            name: "shell".into(),
            arguments: Some(json!({"command":["touch", marker]})),
        })
        .await
        .unwrap();
    let event = wait_for_event(&fixture.conversation, |e| {
        matches!(e, EventMsg::ExecApprovalRequest(_))
    })
    .await;
    let EventMsg::ExecApprovalRequest(request) = event else {
        unreachable!()
    };
    assert!(!marker.exists());
    fixture
        .conversation
        .submit(Op::ExecApproval {
            id: request.effective_approval_id(),
            turn_id: Some(request.turn_id),
            decision: ReviewDecision::Denied,
        })
        .await
        .unwrap();
    let (message, _) = fixture.notice().await;
    assert!(
        message.contains("reject") || message.contains("denied"),
        "{message}"
    );
    wait_for_event(&fixture.conversation, |e| {
        matches!(e, EventMsg::TaskComplete(_))
    })
    .await;
    assert!(!marker.exists(), "denied shell commands must not execute");
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_mcp_catalog_discovery_and_disabled_tools() {
    let fixture = Fixture::new(|config| {
        config.tools_search_tool = true;
        config.mcp_servers.insert("operator_fixture".into(), serde_json::from_value(json!({
            "command": "python3",
            "args": [std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/operator_mcp.py")],
            "disabled_tools": ["disabled_echo"],
            "startup_timeout_sec": 10,
            "tool_timeout_sec": 5,
        })).unwrap());
    }).await;
    fixture.conversation.submit(Op::ListTools).await.unwrap();
    let (inventory, _) = fixture.notice().await;
    let echo = inventory
        .lines()
        .find(|line| line.contains("MCP operator_fixture") && line.contains("echo"))
        .unwrap_or_else(|| panic!("MCP tool missing: {inventory}"))
        .split_whitespace()
        .next()
        .unwrap();
    assert!(
        inventory.contains("operator_fixture/disabled_echo — disabled"),
        "{inventory}"
    );
    let (output, success) = fixture
        .invoke(echo, Some(json!({"text":"operator-mcp-ok"})))
        .await;
    assert_ne!(success, Some(false), "{output}");
    assert!(output.contains("operator-mcp-ok"), "{output}");
    let disabled = echo.strip_suffix("echo").unwrap().to_owned() + "disabled_echo";
    assert_eq!(
        fixture
            .invoke(&disabled, Some(json!({"text":"must-not-run"})))
            .await
            .1,
        Some(false)
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_do_not_bypass_never_approval_for_sandbox_escalation() {
    let fixture = Fixture::new(|config| {
        config.sandbox_policy = SandboxPolicy::ReadOnly;
    }).await;
    let marker = fixture.work.path().join("must-not-exist");
    let (output, _) = fixture.invoke("shell", Some(json!({
        "command": ["touch", marker],
        "sandbox_permissions": "require_escalated",
        "justification": "Fixture must reject escalation",
    }))).await;
    assert!(output.contains("Never") || output.contains("never"), "{output}");
    assert!(!marker.exists());
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn operator_tools_resume_explicitly_queued_user_work_after_completion() {
    use code_core::protocol::InputItem;
    use wiremock::{Mock, ResponseTemplate};
    use wiremock::matchers::{method, path_regex};

    let fixture = Fixture::new(|_| {}).await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(common::load_sse_fixture_with_id(
                "tests/fixtures/completed_template.json", "queued-operator-turn")))
        .mount(&fixture.server).await;
    fixture.conversation.submit(Op::InvokeTool {
        name: "request_user_input".into(),
        arguments: Some(json!({"questions":[{"id":"continue","header":"Test","question":"Continue?"}]})),
    }).await.unwrap();
    let event = wait_for_event(&fixture.conversation, |e| matches!(e, EventMsg::RequestUserInput(_))).await;
    let EventMsg::RequestUserInput(request) = event else { unreachable!() };
    fixture.conversation.submit(Op::QueueUserInput {
        items: vec![InputItem::Text { text: "This queued message alone may contact the fixture provider".into() }],
    }).await.unwrap();
    fixture.conversation.submit(Op::UserInputAnswer {
        id: request.turn_id,
        response: serde_json::from_value(json!({"answers":{"continue":{"answers":["yes"]}}})).unwrap(),
    }).await.unwrap();
    fixture.notice().await;
    wait_for_event(&fixture.conversation, |e| matches!(e, EventMsg::TaskComplete(_))).await;
    wait_for_event(&fixture.conversation, |e| matches!(e, EventMsg::TaskStarted)).await;
    wait_for_event(&fixture.conversation, |e| matches!(e, EventMsg::TaskComplete(_))).await;
    fixture.conversation.submit(Op::Shutdown).await.unwrap();
    wait_for_event(&fixture.conversation, |e| matches!(e, EventMsg::ShutdownComplete)).await;
    let requests = fixture.server.received_requests().await.unwrap();
    let requests = requests.iter().filter(|r| r.url.path().ends_with("/responses")).collect::<Vec<_>>();
    assert_eq!(requests.len(), 1, "only the explicitly queued prompt may contact the provider");
    let body = requests[0].body_json::<Value>().unwrap();
    assert!(!body["input"].as_array().unwrap().iter().any(|item| item["type"] == "function_call_output"));
}
