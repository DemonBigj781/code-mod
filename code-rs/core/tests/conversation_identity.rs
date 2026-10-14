use code_core::config::{Config, ConfigOverrides, ConfigToml};
use code_core::protocol::{EventMsg, Op};
use code_core::{CodexAuth, CodexConversation, ConversationManager, SessionCatalog};
use std::path::Path;
use std::sync::Arc;
use tokio::time::{Duration, timeout};

fn config(home: &Path) -> Config {
    Config::load_from_base_config_with_overrides(
        ConfigToml::default(),
        ConfigOverrides {
            cwd: Some(home.to_path_buf()),
            ..Default::default()
        },
        home.to_path_buf(),
    )
    .expect("isolated test config")
}

async fn shutdown(conversation: &Arc<CodexConversation>) {
    conversation
        .submit(Op::Shutdown)
        .await
        .expect("request shutdown");
    timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                conversation.next_event().await.expect("shutdown event").msg,
                EventMsg::ShutdownComplete
            ) {
                break;
            }
        }
    })
    .await
    .expect("session shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resumed_and_forked_conversations_use_the_configured_identity() {
    let home = tempfile::tempdir().expect("temporary Code home");
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("local-test-only"));
    let first = manager
        .new_conversation(config(home.path()))
        .await
        .expect("new session");
    let original = first.conversation_id;
    assert_eq!(
        original.to_string(),
        first.session_configured.session_id.to_string()
    );
    shutdown(&first.conversation).await;
    manager.remove_conversation(&original).await;

    let catalog = SessionCatalog::new(home.path().to_path_buf());
    let entry = catalog
        .find_by_id(&original.to_string())
        .await
        .expect("read catalog")
        .expect("saved session");
    let path = catalog.entry_rollout_path(&entry);
    let resumed = manager
        .resume_conversation_from_rollout(config(home.path()), path.clone(), manager.auth_manager())
        .await
        .expect("resume saved session");
    let resumed_id = resumed.conversation_id;
    let configured_id = resumed.session_configured.session_id;
    let loaded = manager.loaded_conversation_ids().await;
    shutdown(&resumed.conversation).await;
    assert_eq!(configured_id.to_string(), original.to_string());
    assert_eq!(
        resumed_id, original,
        "resume must not register the provisional spawn UUID"
    );
    assert!(loaded.contains(&original));
    assert!(manager.get_conversation(original).await.is_ok());

    let forked = manager
        .fork_conversation(0, config(home.path()), path)
        .await
        .expect("fork session");
    let forked_id = forked.conversation_id;
    let configured_id = forked.session_configured.session_id;
    shutdown(&forked.conversation).await;
    assert_ne!(forked_id, original);
    assert_eq!(
        forked_id.to_string(),
        configured_id.to_string(),
        "fork routing must match its new rollout identity"
    );
    assert!(manager.loaded_conversation_ids().await.contains(&forked_id));
}
