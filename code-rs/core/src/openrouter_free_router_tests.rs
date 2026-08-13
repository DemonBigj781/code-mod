use crate::openrouter_free_router::FreeModelCache;
use crate::openrouter_free_router::OpenRouterFreeCandidate;
use crate::openrouter_free_router::catalog_status_allows_stale_fallback;
use crate::openrouter_free_router::eligible_candidates;
use crate::openrouter_free_router::read_cache;
use crate::openrouter_free_router::write_cache;

use reqwest::StatusCode;
use serde_json::json;

#[test]
fn filters_and_orders_free_tool_capable_models() {
    let catalog = json!({
        "data": [
            {
                "id": "vendor/tie-b:free",
                "context_length": 200_000,
                "pricing": { "prompt": "0", "completion": "0" },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"],
                "top_provider": { "max_completion_tokens": 8_192 }
            },
            {
                "id": "vendor/largest:free",
                "context_length": 300_000,
                "pricing": { "prompt": 0, "completion": 0 },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"],
                "top_provider": { "max_completion_tokens": 4_096 }
            },
            {
                "id": "vendor/tie-a:free",
                "context_length": 200_000,
                "pricing": { "prompt": "0.0", "completion": "0.000" },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"],
                "top_provider": { "max_completion_tokens": 8_192 }
            },
            {
                "id": "vendor/paid:free",
                "context_length": 1_000_000,
                "pricing": { "prompt": "0.1", "completion": "0" },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"]
            },
            {
                "id": "vendor/request-fee:free",
                "context_length": 950_000,
                "pricing": {
                    "prompt": "0",
                    "completion": "0",
                    "request": "0.001"
                },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"]
            },
            {
                "id": "vendor/no-tools:free",
                "context_length": 900_000,
                "pricing": { "prompt": "0", "completion": "0" },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["temperature"]
            },
            {
                "id": "vendor/image-only:free",
                "context_length": 800_000,
                "pricing": { "prompt": "0", "completion": "0" },
                "architecture": {
                    "input_modalities": ["image"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"]
            },
            {
                "id": "vendor/not-free",
                "context_length": 700_000,
                "pricing": { "prompt": "0", "completion": "0" },
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "tool_choice"]
            }
        ]
    });

    let candidates = eligible_candidates(catalog).expect("catalog should parse");

    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "vendor/largest:free",
            "vendor/tie-a:free",
            "vendor/tie-b:free"
        ]
    );
}

#[test]
fn stale_cache_is_only_allowed_for_transient_catalog_statuses() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::PAYMENT_REQUIRED,
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::NOT_FOUND,
    ] {
        assert!(!catalog_status_allows_stale_fallback(status));
    }

    for status in [StatusCode::REQUEST_TIMEOUT, StatusCode::BAD_GATEWAY] {
        assert!(catalog_status_allows_stale_fallback(status));
    }
}

#[test]
fn cache_freshness_expires_after_twenty_four_hours() {
    let cache = FreeModelCache::new(10_000, Vec::new());

    assert!(cache.is_fresh(10_000 + 24 * 60 * 60 - 1));
    assert!(!cache.is_fresh(10_000 + 24 * 60 * 60));
}

#[tokio::test]
async fn cache_preserves_ranked_catalog_without_credentials() {
    let temp = tempfile::tempdir().expect("temporary directory should be created");
    let path = temp.path().join("cache.json");
    let cache = FreeModelCache::new(
        123,
        vec![OpenRouterFreeCandidate {
            id: "vendor/model:free".to_string(),
            context_length: 200_000,
            max_completion_tokens: 8_192,
        }],
    );

    write_cache(&path, &cache)
        .await
        .expect("cache should be written");
    let restored = read_cache(&path)
        .await
        .expect("cache should be readable")
        .expect("cache should be present");

    assert_eq!(restored, cache);
    let contents = std::fs::read_to_string(path).expect("cache should be readable as text");
    assert!(!contents.contains("api_key"));
    assert!(!contents.contains("OPENROUTER_API_KEY"));
}
