use crate::CodexAuth;
use crate::model_provider_info::ModelProviderInfo;

use reqwest::Method;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Mutex;

pub(crate) const OPENROUTER_FREE_MAX_MODEL: &str = "openrouter/free-max";
const CACHE_VERSION: u32 = 1;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CACHE_RELATIVE_PATH: &str = "cache/openrouter-free-max.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OpenRouterFreeCandidate {
    pub(crate) id: String,
    pub(crate) context_length: u64,
    pub(crate) max_completion_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct FreeModelCache {
    version: u32,
    fetched_at_unix_seconds: u64,
    pub(crate) candidates: Vec<OpenRouterFreeCandidate>,
}

impl FreeModelCache {
    pub(crate) fn new(
        fetched_at_unix_seconds: u64,
        candidates: Vec<OpenRouterFreeCandidate>,
    ) -> Self {
        Self {
            version: CACHE_VERSION,
            fetched_at_unix_seconds,
            candidates,
        }
    }

    pub(crate) fn is_fresh(&self, now_unix_seconds: u64) -> bool {
        self.version == CACHE_VERSION
            && now_unix_seconds.saturating_sub(self.fetched_at_unix_seconds) < CACHE_TTL.as_secs()
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterCatalog {
    data: Vec<OpenRouterCatalogModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterCatalogModel {
    id: String,
    context_length: Option<u64>,
    pricing: OpenRouterPricing,
    architecture: OpenRouterArchitecture,
    #[serde(default)]
    supported_parameters: Vec<String>,
    top_provider: Option<OpenRouterTopProvider>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterPricing {
    prompt: Value,
    completion: Value,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
    #[serde(default)]
    output_modalities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterTopProvider {
    max_completion_tokens: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct OpenRouterFreeRouter {
    cache_path: PathBuf,
    cache: Mutex<Option<FreeModelCache>>,
}

#[derive(Debug)]
enum CatalogRefreshError {
    Fatal(String),
    Transient(String),
}

impl CatalogRefreshError {
    fn into_message(self) -> String {
        match self {
            Self::Fatal(message) | Self::Transient(message) => message,
        }
    }
}

impl OpenRouterFreeRouter {
    pub(crate) fn new(code_home: &Path) -> Self {
        Self {
            cache_path: code_home.join(CACHE_RELATIVE_PATH),
            cache: Mutex::new(None),
        }
    }

    pub(crate) async fn candidate_ids(
        &self,
        provider: &ModelProviderInfo,
        auth: &Option<CodexAuth>,
        client: &reqwest::Client,
    ) -> Result<Vec<String>, String> {
        let now = unix_seconds(SystemTime::now()).map_err(|error| error.to_string())?;
        let mut cache_guard = self.cache.lock().await;

        if cache_guard.is_none() {
            *cache_guard = read_cache(&self.cache_path)
                .await
                .map_err(|error| error.to_string())?;
        }

        if let Some(cache) = cache_guard.as_ref()
            && cache.is_fresh(now)
            && !cache.candidates.is_empty()
        {
            return Ok(candidate_ids(&cache.candidates));
        }

        let stale = cache_guard
            .as_ref()
            .filter(|cache| !cache.candidates.is_empty())
            .cloned();
        match refresh(provider, auth, client, now).await {
            Ok(cache) if !cache.candidates.is_empty() => {
                if let Err(error) = write_cache(&self.cache_path, &cache).await {
                    tracing::warn!(%error, "failed to persist OpenRouter free-max cache");
                }
                let ids = candidate_ids(&cache.candidates);
                *cache_guard = Some(cache);
                Ok(ids)
            }
            Ok(cache) => {
                *cache_guard = Some(cache);
                Err("OpenRouter has no compatible zero-price free models".to_owned())
            }
            Err(CatalogRefreshError::Transient(error)) => {
                if let Some(stale) = stale {
                    tracing::warn!(%error, "using stale OpenRouter free-max cache after refresh failure");
                    let ids = candidate_ids(&stale.candidates);
                    *cache_guard = Some(stale);
                    Ok(ids)
                } else {
                    Err(error)
                }
            }
            Err(error @ CatalogRefreshError::Fatal(_)) => Err(error.into_message()),
        }
    }
}

fn candidate_ids(candidates: &[OpenRouterFreeCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect()
}

pub(crate) fn eligible_candidates(
    catalog: Value,
) -> serde_json::Result<Vec<OpenRouterFreeCandidate>> {
    let catalog: OpenRouterCatalog = serde_json::from_value(catalog)?;
    let mut candidates = catalog
        .data
        .into_iter()
        .filter(is_eligible)
        .map(|model| OpenRouterFreeCandidate {
            id: model.id,
            context_length: model.context_length.unwrap_or_default(),
            max_completion_tokens: model
                .top_provider
                .and_then(|provider| provider.max_completion_tokens)
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .context_length
            .cmp(&left.context_length)
            .then_with(|| right.max_completion_tokens.cmp(&left.max_completion_tokens))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(candidates)
}

fn is_eligible(model: &OpenRouterCatalogModel) -> bool {
    model.id.contains('/')
        && model.id.ends_with(":free")
        && is_zero_pricing(&model.pricing)
        && model
            .architecture
            .input_modalities
            .iter()
            .any(|modality| modality == "text")
        && model
            .architecture
            .output_modalities
            .iter()
            .any(|modality| modality == "text")
        && model
            .supported_parameters
            .iter()
            .any(|parameter| parameter == "tools")
        && model
            .supported_parameters
            .iter()
            .any(|parameter| parameter == "tool_choice")
}

fn is_zero_pricing(pricing: &OpenRouterPricing) -> bool {
    is_zero_price(&pricing.prompt)
        && is_zero_price(&pricing.completion)
        && pricing.extra.values().all(is_zero_price)
}

fn is_zero_price(value: &Value) -> bool {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        == Some(0.0)
}

pub(crate) fn catalog_status_allows_stale_fallback(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT || status.is_server_error()
}

async fn refresh(
    provider: &ModelProviderInfo,
    auth: &Option<CodexAuth>,
    client: &reqwest::Client,
    now_unix_seconds: u64,
) -> Result<FreeModelCache, CatalogRefreshError> {
    let base_url = provider.base_url.as_deref().ok_or_else(|| {
        CatalogRefreshError::Fatal("OpenRouter provider has no base URL".to_owned())
    })?;
    let models_url = reqwest::Url::parse(&format!("{}/models", base_url.trim_end_matches('/')))
        .map_err(|error| {
            CatalogRefreshError::Fatal(format!("invalid OpenRouter catalog URL: {error}"))
        })?;
    let response = provider
        .create_request_builder_for_url_with_auth(client, auth.as_ref(), Method::GET, models_url)
        .await
        .map_err(|error| {
            CatalogRefreshError::Fatal(format!(
                "failed to build OpenRouter catalog request: {error}"
            ))
        })?
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            CatalogRefreshError::Transient(format!(
                "failed to fetch OpenRouter model catalog: {error}"
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let message = format!("OpenRouter model catalog returned HTTP {}", status.as_u16());
        return Err(if catalog_status_allows_stale_fallback(status) {
            CatalogRefreshError::Transient(message)
        } else {
            CatalogRefreshError::Fatal(message)
        });
    }
    let catalog = response.json::<Value>().await.map_err(|error| {
        CatalogRefreshError::Transient(format!("invalid OpenRouter model catalog: {error}"))
    })?;
    let candidates = eligible_candidates(catalog).map_err(|error| {
        CatalogRefreshError::Transient(format!("invalid OpenRouter model metadata: {error}"))
    })?;
    Ok(FreeModelCache::new(now_unix_seconds, candidates))
}

pub(crate) async fn read_cache(path: &Path) -> io::Result<Option<FreeModelCache>> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match serde_json::from_str::<FreeModelCache>(&contents) {
        Ok(cache) if cache.version == CACHE_VERSION => Ok(Some(cache)),
        Ok(_) => Ok(None),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "ignoring malformed OpenRouter free-max cache");
            Ok(None)
        }
    }
}

pub(crate) async fn write_cache(path: &Path, cache: &FreeModelCache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let temp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let contents = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    tokio::fs::write(&temp_path, contents).await?;
    tokio::fs::rename(&temp_path, path).await
}

fn unix_seconds(time: SystemTime) -> io::Result<u64> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}
