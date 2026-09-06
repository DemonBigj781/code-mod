use super::auth::CoreRemoteControlAuthProvider;
use super::auth::REMOTE_CONTROL_ACCOUNT_ID_HEADER;
use super::auth::RemoteControlAuthProvider;
use base64::Engine;
use chrono::Utc;
use code_app_server_protocol::AuthMode;
use code_core::auth::AuthDotJson;
use code_core::auth::AuthManager;
use code_core::auth::CodexAuth;
use code_core::auth::AuthCredentialsStoreMode;
use code_core::token_data::IdTokenInfo;
use code_core::token_data::TokenData;
use code_core::token_data::parse_id_token;
use reqwest::header::AUTHORIZATION;
use std::sync::Arc;

fn chatgpt_tokens(access_token: &str, account_id: Option<&str>) -> TokenData {
    TokenData {
        id_token: test_id_token(),
        access_token: access_token.to_string(),
        refresh_token: "refresh-token".to_string(),
        account_id: account_id.map(str::to_string),
    }
}

fn test_id_token() -> IdTokenInfo {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header = encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = encode(
        br#"{"email":"remote@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"account-a","chatgpt_plan_type":"plus","chatgpt_user_id":"user-a"}}"#,
    );
    parse_id_token(&format!("{header}.{payload}.sig")).expect("parse test ID token")
}

#[tokio::test]
async fn loads_chatgpt_auth_and_builds_redaction_safe_headers() {
    let manager = AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let provider = CoreRemoteControlAuthProvider::new(manager);

    let auth = provider.load().await.expect("load ChatGPT auth");
    let headers = auth.request_headers().expect("build request headers");
    let authorization = headers
        .get(AUTHORIZATION)
        .expect("authorization header must exist");
    assert!(
        authorization.as_bytes() == b"Bearer Access Token",
        "authorization header mismatch"
    );
    assert_eq!(
        headers
            .get(REMOTE_CONTROL_ACCOUNT_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("account_id")
    );
    assert!(!format!("{auth:?}").contains("Access Token"));
}

#[tokio::test]
async fn rejects_api_key_authentication() {
    let manager = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("secret-api-key"));
    let provider = CoreRemoteControlAuthProvider::new(manager);

    let error = provider
        .load()
        .await
        .expect_err("API key auth must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(!error.to_string().contains("secret-api-key"));
}

#[tokio::test]
async fn reloads_once_when_the_cached_auth_has_no_account_id() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let cached = CodexAuth::from_tokens_with_originator(
        chatgpt_tokens("cached-token", None),
        Some(Utc::now()),
        "remote-control-test",
    );
    let persisted = AuthDotJson {
        auth_mode: Some(AuthMode::ChatGPT),
        openai_api_key: None,
        tokens: Some(chatgpt_tokens("persisted-token", Some("account-a"))),
        last_refresh: Some(Utc::now()),
    };
    std::fs::write(
        code_home.path().join("auth.json"),
        serde_json::to_vec_pretty(&persisted).expect("serialize persisted auth"),
    )
    .expect("write persisted auth");
    let manager = AuthManager::from_auth(
        cached,
        code_home.path().to_path_buf(),
        "remote-control-test".to_string(),
        AuthCredentialsStoreMode::File,
    );
    let provider = CoreRemoteControlAuthProvider::new(manager);

    let auth = provider.load().await.expect("reload ChatGPT auth");
    assert_eq!(auth.account_id(), "account-a");
}

#[tokio::test]
async fn concurrent_unauthorized_recovery_reuses_one_auth_change() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let initial = CodexAuth::from_tokens_with_originator(
        chatgpt_tokens("initial-token", Some("account-a")),
        Some(Utc::now()),
        "remote-control-test",
    );
    let manager = AuthManager::from_auth(
        initial,
        code_home.path().to_path_buf(),
        "remote-control-test".to_string(),
        AuthCredentialsStoreMode::File,
    );
    let provider = Arc::new(CoreRemoteControlAuthProvider::new(Arc::clone(&manager)));
    provider.load().await.expect("load initial auth");

    let rotated = AuthDotJson {
        auth_mode: Some(AuthMode::ChatGPT),
        openai_api_key: None,
        tokens: Some(chatgpt_tokens("rotated-token", Some("account-a"))),
        last_refresh: Some(Utc::now()),
    };
    std::fs::write(
        code_home.path().join("auth.json"),
        serde_json::to_vec_pretty(&rotated).expect("serialize rotated auth"),
    )
    .expect("write rotated auth");

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let provider = Arc::clone(&provider);
        tasks.push(tokio::spawn(async move {
            provider
                .recover_unauthorized()
                .await
                .expect("recover unauthorized request")
        }));
    }
    for task in tasks {
        assert!(task.await.expect("join recovery task"));
    }

    assert_eq!(manager.auth_revision(), 1);
    let token = manager
        .auth()
        .expect("auth remains available")
        .get_token()
        .await
        .expect("load rotated token");
    assert!(token == "rotated-token", "rotated token was not adopted");
}
