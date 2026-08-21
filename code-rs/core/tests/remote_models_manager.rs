use chrono::Utc;
use code_core::AuthManager;
use code_core::CodexAuth;
use code_core::ModelProviderInfo;
use code_core::WireApi;
use code_core::remote_models::RemoteModelsManager;
use code_core::remote_models::RemoteModelsStatus;
use code_protocol::openai_models::ModelInfo;
use code_protocol::openai_models::ModelsResponse;
use pretty_assertions::assert_eq;
use tempfile::tempdir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn skip_if_no_network() -> bool {
    std::env::var(code_core::spawn::CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR).is_ok()
}

fn remote_model(slug: &str, display: &str, priority: i32) -> TestResult<ModelInfo> {
    Ok(serde_json::from_value(serde_json::json!({
        "slug": slug,
        "display_name": display,
        "description": format!("{display} desc"),
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "low"},
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": priority,
        "upgrade": null,
        "base_instructions": "",
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10_000},
        "supports_parallel_tool_calls": false,
        "context_window": null,
        "experimental_supported_tools": [],
    }))?)
}

fn provider_for(base_url: String) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "mock".into(),
        base_url: Some(base_url),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        openrouter: None,
    }
}

fn standard_models(ids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "object": "list",
        "data": ids
            .iter()
            .map(|id| serde_json::json!({
                "id": id,
                "object": "model",
                "created": 0,
                "owned_by": "test",
            }))
            .collect::<Vec<_>>(),
    })
}

fn provider_cache_files(code_home: &std::path::Path) -> TestResult<Vec<std::path::PathBuf>> {
    let mut files = std::fs::read_dir(code_home)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("models_cache-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn auth_manager_chatgpt() -> std::sync::Arc<AuthManager> {
    AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing())
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

#[tokio::test]
async fn refresh_remote_models_uses_cache_when_fresh() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    let response = ModelsResponse {
        models: vec![remote_model("cached", "Cached", 1)?],
    };

    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&response)
                .insert_header("ETag", "etag-1"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(server.uri());
    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider,
        code_home.path().to_path_buf(),
    );

    manager.refresh_remote_models().await;
    let models = manager.remote_models_snapshot().await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "cached");

    let requests = server
        .received_requests()
        .await
        .ok_or_else(|| std::io::Error::other("request log unavailable"))?;
    assert_eq!(requests.len(), 1);
    let request_url = requests[0].url.as_str();
    let client_version = query_param(request_url, "client_version")
        .ok_or_else(|| std::io::Error::other("client_version query param missing"))?;
    assert_eq!(
        client_version,
        code_version::wire_compatible_version(),
        "expected client_version query param in {request_url}"
    );

    // Second refresh should hit the fresh in-memory snapshot and avoid the network.
    manager.refresh_remote_models().await;
    let requests = server
        .received_requests()
        .await
        .ok_or_else(|| std::io::Error::other("request log unavailable"))?;
    assert_eq!(requests.len(), 1);
    Ok(())
}

#[tokio::test]
async fn refresh_remote_models_refetches_when_cache_stale() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    let initial = ModelsResponse {
        models: vec![remote_model("stale", "Stale", 1)?],
    };

    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&initial)
                .insert_header("ETag", "etag-stale"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(server.uri());

    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider.clone(),
        code_home.path().to_path_buf(),
    );
    manager.refresh_remote_models().await;
    assert_eq!(manager.remote_models_snapshot().await[0].slug, "stale");

    // Rewrite the cache to be stale.
    let cache_path = code_home.path().join("models_cache.json");
    let contents = std::fs::read_to_string(&cache_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&contents)?;
    let old = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    json["fetched_at"] = serde_json::Value::String(old);
    std::fs::write(&cache_path, serde_json::to_vec_pretty(&json)?)?;

    let updated = ModelsResponse {
        models: vec![remote_model("fresh", "Fresh", 0)?],
    };

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&updated)
                .insert_header("ETag", "etag-fresh"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // New manager should load the stale cache and refetch.
    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider,
        code_home.path().to_path_buf(),
    );
    manager.refresh_remote_models().await;
    let models = manager.remote_models_snapshot().await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "fresh");
    assert_eq!(
        server
            .received_requests()
            .await
            .ok_or_else(|| std::io::Error::other("request log unavailable"))?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn refresh_remote_models_sends_if_none_match_and_handles_304() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    let initial = ModelsResponse {
        models: vec![remote_model("cached", "Cached", 1)?],
    };

    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&initial)
                .insert_header("ETag", "etag-304"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(server.uri());
    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider.clone(),
        code_home.path().to_path_buf(),
    );
    manager.refresh_remote_models().await;

    // Rewrite cache to be stale.
    let cache_path = code_home.path().join("models_cache.json");
    let contents = std::fs::read_to_string(&cache_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&contents)?;
    let old = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    json["fetched_at"] = serde_json::Value::String(old);
    std::fs::write(&cache_path, serde_json::to_vec_pretty(&json)?)?;

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("if-none-match", "etag-304"))
        .respond_with(ResponseTemplate::new(304))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider,
        code_home.path().to_path_buf(),
    );
    manager.refresh_remote_models().await;
    let models = manager.remote_models_snapshot().await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "cached");
    Ok(())
}

#[tokio::test]
async fn construct_model_family_applies_remote_overrides() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    let info: ModelInfo = serde_json::from_value(serde_json::json!({
        "slug": "gpt-5.2",
        "display_name": "gpt-5.2",
        "description": null,
        "default_reasoning_level": "high",
        "supported_reasoning_levels": [],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 0,
        "upgrade": null,
        "base_instructions": "REMOTE INSTRUCTIONS",
        "supports_reasoning_summaries": true,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": "function",
        "truncation_policy": {"mode": "bytes", "limit": 10_000},
        "supports_parallel_tool_calls": false,
        "context_window": 12345,
        "experimental_supported_tools": [],
    }))?;

    let response = ModelsResponse {
        models: vec![info],
    };

    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(server.uri());
    let manager = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider,
        code_home.path().to_path_buf(),
    );

    manager.refresh_remote_models().await;
    let family = manager.construct_model_family("gpt-5.2").await;
    assert_eq!(family.context_window, Some(12345));
    assert_eq!(family.base_instructions, "REMOTE INSTRUCTIONS");
    assert_eq!(
        family.apply_patch_tool_type,
        Some(code_core::ApplyPatchToolType::Function)
    );
    assert_eq!(family.supports_reasoning_summaries, true);
    assert_eq!(
        family.default_reasoning_effort,
        Some(code_core::config_types::ReasoningEffort::High)
    );
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_equal_model_ids_remain_isolated() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let first_server = MockServer::start().await;
    let second_server = MockServer::start().await;
    for server in [&first_server, &second_server] {
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["shared-model"])))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }

    let code_home = tempdir()?;
    let first = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-first",
        provider_for(format!("{}/v1", first_server.uri())),
        code_home.path().to_path_buf(),
    );
    let second = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-second",
        provider_for(format!("{}/v1", second_server.uri())),
        code_home.path().to_path_buf(),
    );

    let first_catalog = first.refresh_remote_models().await;
    let second_catalog = second.refresh_remote_models().await;

    assert_eq!(first_catalog.provider_id, "direct-first");
    assert_eq!(second_catalog.provider_id, "direct-second");
    assert_eq!(first_catalog.models[0].slug, "shared-model");
    assert_eq!(second_catalog.models[0].slug, "shared-model");
    assert_eq!(first_catalog.status, RemoteModelsStatus::Fresh);
    assert_eq!(second_catalog.status, RemoteModelsStatus::Fresh);
    for server in [&first_server, &second_server] {
        let requests = server
            .received_requests()
            .await
            .ok_or_else(|| std::io::Error::other("request log unavailable"))?;
        assert_eq!(requests.len(), 1);
        assert_eq!(query_param(requests[0].url.as_str(), "client_version"), None);
    }
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_cache_writes_are_isolated() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let first_server = MockServer::start().await;
    let second_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["first-model"])))
        .up_to_n_times(1)
        .mount(&first_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["second-model"])))
        .up_to_n_times(1)
        .mount(&second_server)
        .await;

    let code_home = tempdir()?;
    let first = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-first",
        provider_for(format!("{}/v1", first_server.uri())),
        code_home.path().to_path_buf(),
    );
    let second = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-second",
        provider_for(format!("{}/v1", second_server.uri())),
        code_home.path().to_path_buf(),
    );
    let _ = first.refresh_remote_models().await;
    let _ = second.refresh_remote_models().await;

    let files = provider_cache_files(code_home.path())?;
    assert_eq!(files.len(), 2);
    assert!(!code_home.path().join("models_cache.json").exists());
    let mut cached_slugs = files
        .iter()
        .map(|path| -> TestResult<String> {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path)?)?;
            let slug = value["models"][0]["slug"]
                .as_str()
                .ok_or_else(|| std::io::Error::other("cached model slug missing"))?;
            Ok(slug.to_owned())
        })
        .collect::<TestResult<Vec<_>>>()?;
    cached_slugs.sort();
    assert_eq!(cached_slugs, vec!["first-model", "second-model"]);
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_auth_failure_retains_stale_models() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["cached-model"])))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(format!("{}/v1", server.uri()));
    let manager = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-auth",
        provider.clone(),
        code_home.path().to_path_buf(),
    );
    assert_eq!(
        manager.refresh_remote_models().await.status,
        RemoteModelsStatus::Fresh
    );

    let cache_path = provider_cache_files(code_home.path())?
        .into_iter()
        .next()
        .ok_or_else(|| std::io::Error::other("provider cache missing"))?;
    let mut cache: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache_path)?)?;
    cache["fetched_at"] = serde_json::Value::String(
        (Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
    );
    std::fs::write(&cache_path, serde_json::to_vec_pretty(&cache)?)?;

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let manager = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-auth",
        provider,
        code_home.path().to_path_buf(),
    );
    let catalog = manager.refresh_remote_models().await;
    assert!(matches!(
        catalog.status,
        RemoteModelsStatus::AuthenticationError { .. }
    ));
    assert_eq!(catalog.models[0].slug, "cached-model");
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_refresh_leaves_other_provider_unchanged() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let first_server = MockServer::start().await;
    let second_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["first-model"])))
        .up_to_n_times(1)
        .mount(&first_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(standard_models(&["second-model"])))
        .up_to_n_times(1)
        .mount(&second_server)
        .await;

    let code_home = tempdir()?;
    let first = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-first",
        provider_for(format!("{}/v1", first_server.uri())),
        code_home.path().to_path_buf(),
    );
    let second = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-second",
        provider_for(format!("{}/v1", second_server.uri())),
        code_home.path().to_path_buf(),
    );
    let _ = first.refresh_remote_models().await;
    let second_before = second.refresh_remote_models().await;

    first_server.reset().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&first_server)
        .await;

    let first_after = first.refresh_remote_models_no_cache().await;
    let second_after = second.catalog_snapshot().await;
    assert!(matches!(
        first_after.status,
        RemoteModelsStatus::ConnectionError { .. }
    ));
    assert_eq!(first_after.models[0].slug, "first-model");
    assert_eq!(second_after, second_before);
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_reports_loading_while_refresh_is_in_flight() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(1))
                .set_body_json(standard_models(&["delayed-model"])),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let manager = std::sync::Arc::new(RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "direct-loading",
        provider_for(format!("{}/v1", server.uri())),
        code_home.path().to_path_buf(),
    ));
    assert_eq!(
        manager.catalog_snapshot().await.status,
        RemoteModelsStatus::Stale
    );

    let refresh_manager = std::sync::Arc::clone(&manager);
    let refresh = tokio::spawn(async move { refresh_manager.refresh_remote_models().await });
    for _ in 0..50 {
        if server
            .received_requests()
            .await
            .is_some_and(|requests| !requests.is_empty())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(
        manager.catalog_snapshot().await.status,
        RemoteModelsStatus::Loading
    );
    let catalog = refresh.await?;
    assert_eq!(catalog.status, RemoteModelsStatus::Fresh);
    assert_eq!(catalog.models[0].slug, "delayed-model");
    Ok(())
}

#[tokio::test]
async fn remote_models_provider_openai_loads_legacy_cache() -> TestResult {
    if skip_if_no_network() {
        return Ok(());
    }

    let server = MockServer::start().await;
    let response = ModelsResponse {
        models: vec![remote_model("legacy-model", "Legacy", 1)?],
    };
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let code_home = tempdir()?;
    let provider = provider_for(server.uri());
    let first = RemoteModelsManager::new(
        auth_manager_chatgpt(),
        provider.clone(),
        code_home.path().to_path_buf(),
    );
    let _ = first.refresh_remote_models().await;
    assert!(code_home.path().join("models_cache.json").exists());

    let second = RemoteModelsManager::new_for_provider(
        auth_manager_chatgpt(),
        "openai",
        provider,
        code_home.path().to_path_buf(),
    );
    let catalog = second.refresh_remote_models().await;
    assert_eq!(catalog.status, RemoteModelsStatus::Fresh);
    assert_eq!(catalog.models[0].slug, "legacy-model");
    assert_eq!(
        server
            .received_requests()
            .await
            .ok_or_else(|| std::io::Error::other("request log unavailable"))?
            .len(),
        1
    );
    Ok(())
}
