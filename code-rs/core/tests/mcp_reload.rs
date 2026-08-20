#![allow(clippy::unwrap_used)]

mod common;

use code_core::CodexAuth;
use code_core::CodexConversation;
use code_core::ConversationManager;
use code_core::protocol::EventMsg;
use code_core::protocol::Op;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

async fn new_conversation(
    manager: &ConversationManager,
    code_home: &TempDir,
) -> std::sync::Arc<CodexConversation> {
    let mut config = common::load_default_config_for_test(code_home);
    config.cwd = code_home.path().to_path_buf();
    manager
        .new_conversation(config)
        .await
        .expect("create conversation")
        .conversation
}

async fn wait_for_mcp_snapshot(conversation: &CodexConversation) {
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                conversation.next_event().await.expect("next event").msg,
                EventMsg::McpListToolsResponse(_)
            ) {
                break;
            }
        }
    })
    .await
    .expect("MCP snapshot should arrive");
}

async fn wait_for_error(conversation: &CodexConversation) -> code_core::protocol::ErrorEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            if let EventMsg::Error(error) = conversation.next_event().await.expect("next event").msg
            {
                break error;
            }
        }
    })
    .await
    .expect("error event should arrive")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn reload_mcp_unknown_server_emits_error_then_fresh_snapshot() {
    let code_home = TempDir::new().unwrap();
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"));
    let conversation = new_conversation(&manager, &code_home).await;

    conversation
        .submit(Op::ReloadMcpServers {
            server: Some("missing".to_owned()),
        })
        .await
        .expect("submit reload");

    let error = wait_for_error(&conversation).await;
    assert!(error.message.contains("unknown MCP server 'missing'"));
    wait_for_mcp_snapshot(&conversation).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn startup_disabled_mcp_can_enable_then_disable_on_the_live_manager() {
    let code_home = TempDir::new().unwrap();
    std::fs::write(
        code_home.path().join("config.toml"),
        r#"[mcp_servers_disabled.alpha]
command = "missing-live-enable-command"
"#,
    )
    .expect("write disabled MCP config");
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"));
    let conversation = new_conversation(&manager, &code_home).await;

    conversation
        .submit(Op::SetMcpServerEnabled {
            server: "alpha".to_owned(),
            enabled: true,
        })
        .await
        .expect("submit enable");

    let error = wait_for_error(&conversation).await;
    assert!(error.message.contains("missing-live-enable-command"));
    assert!(!error.message.contains("unknown disabled MCP server"));
    wait_for_mcp_snapshot(&conversation).await;

    conversation
        .submit(Op::SetMcpServerEnabled {
            server: "alpha".to_owned(),
            enabled: false,
        })
        .await
        .expect("submit disable");
    wait_for_mcp_snapshot(&conversation).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn reload_mcp_all_fans_out_to_every_loaded_conversation() {
    let code_home = TempDir::new().unwrap();
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("Test API Key"));
    let first = new_conversation(&manager, &code_home).await;
    let second = new_conversation(&manager, &code_home).await;

    manager.reload_mcp_servers().await.expect("fan out reload");

    wait_for_mcp_snapshot(&first).await;
    wait_for_mcp_snapshot(&second).await;
}
