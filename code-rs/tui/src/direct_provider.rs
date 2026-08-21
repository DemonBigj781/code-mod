use std::path::PathBuf;
use std::sync::Arc;

use code_core::config::service::ConfigService;
use code_core::remote_models::{RemoteModelsCatalog, RemoteModelsManager, RemoteModelsStatus};
use code_core::{
    AuthManager, ModelProviderInfo, WireApi, direct_model_provider_id,
    direct_model_provider_secret_name, normalize_direct_provider_base_url,
};
use code_protocol::openai_models::ModelInfo;

pub(crate) struct DirectProviderRequest {
    pub(crate) display_name: String,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) wire_api: WireApi,
}

#[derive(Debug)]
pub(crate) struct DirectProviderAddOutcome {
    pub(crate) provider_id: String,
    pub(crate) models: Vec<ModelInfo>,
}

struct PreparedDirectProvider {
    provider_id: String,
    provider: ModelProviderInfo,
    secret: Option<(code_secrets::SecretName, String)>,
}

struct StagedSecret {
    manager: code_secrets::SecretsManager,
    name: code_secrets::SecretName,
    previous: Option<String>,
}

impl StagedSecret {
    fn rollback(self) -> Result<(), String> {
        match self.previous {
            Some(previous) => self
                .manager
                .set(&code_secrets::SecretScope::Global, &self.name, &previous)
                .map_err(|error| format!("failed to restore the previous endpoint key: {error}")),
            None => self
                .manager
                .delete(&code_secrets::SecretScope::Global, &self.name)
                .map(|_| ())
                .map_err(|error| format!("failed to remove the rejected endpoint key: {error}")),
        }
    }
}

fn prepare_direct_provider(
    request: DirectProviderRequest,
) -> Result<PreparedDirectProvider, String> {
    let display_name = request.display_name.trim();
    if display_name.is_empty() {
        return Err("Display name is required.".to_owned());
    }
    let base_url = normalize_direct_provider_base_url(&request.base_url)?;
    let wire_api = match request.wire_api {
        WireApi::Chat => WireApi::Chat,
        WireApi::Responses | WireApi::ResponsesWebsocket => WireApi::Responses,
    };
    let provider_id = direct_model_provider_id(display_name, &base_url);
    let api_key = request
        .api_key
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let secret_name = api_key
        .as_ref()
        .map(|_| direct_model_provider_secret_name(&provider_id));
    let provider = ModelProviderInfo::direct_openai_compatible(
        display_name,
        base_url,
        secret_name.clone(),
        wire_api,
    );
    let secret = match (secret_name, api_key) {
        (Some(name), Some(value)) => Some((
            code_secrets::SecretName::new(&name)
                .map_err(|error| format!("invalid endpoint key reference: {error}"))?,
            value,
        )),
        _ => None,
    };

    Ok(PreparedDirectProvider {
        provider_id,
        provider,
        secret,
    })
}

fn stage_secret(
    manager: code_secrets::SecretsManager,
    prepared: &PreparedDirectProvider,
) -> Result<Option<StagedSecret>, String> {
    let Some((name, value)) = prepared.secret.as_ref() else {
        return Ok(None);
    };
    let scope = code_secrets::SecretScope::Global;
    let previous = manager
        .get(&scope, name)
        .map_err(|error| format!("failed to read the existing endpoint key: {error}"))?;
    manager
        .set(&scope, name, value)
        .map_err(|error| format!("failed to store the endpoint key: {error}"))?;
    Ok(Some(StagedSecret {
        manager,
        name: name.clone(),
        previous,
    }))
}

fn persist_provider(
    service: &ConfigService,
    prepared: &PreparedDirectProvider,
) -> Result<(), String> {
    service
        .write_model_provider(&prepared.provider_id, &prepared.provider)
        .map(|_| ())
        .map_err(|error| format!("failed to save endpoint configuration: {error}"))
}

fn catalog_error(catalog: &RemoteModelsCatalog) -> Option<String> {
    match &catalog.status {
        RemoteModelsStatus::Fresh if catalog.models.is_empty() => {
            Some("The endpoint returned no models.".to_owned())
        }
        RemoteModelsStatus::Fresh => None,
        RemoteModelsStatus::Loading => Some("The endpoint is still loading models.".to_owned()),
        RemoteModelsStatus::Stale => {
            Some("The endpoint could not refresh its cached model list.".to_owned())
        }
        RemoteModelsStatus::AuthenticationError { message } => {
            Some(format!("Authentication failed: {message}"))
        }
        RemoteModelsStatus::ConnectionError { message } => {
            Some(format!("Connection failed: {message}"))
        }
    }
}

async fn rollback_staged_secret(staged: Option<StagedSecret>) -> Result<(), String> {
    let Some(staged) = staged else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || staged.rollback())
        .await
        .map_err(|error| format!("endpoint key rollback task failed: {error}"))?
}

pub(crate) async fn add_direct_provider(
    request: DirectProviderRequest,
    code_home: PathBuf,
    cwd: PathBuf,
    auth_manager: Arc<AuthManager>,
) -> Result<DirectProviderAddOutcome, String> {
    let prepared = prepare_direct_provider(request)?;
    let secrets = code_secrets::SecretsManager::new(
        code_home.clone(),
        code_secrets::SecretsBackendKind::Local,
    );
    let staged = stage_secret(secrets, &prepared)?;

    let remote_manager = RemoteModelsManager::new_for_provider(
        auth_manager,
        prepared.provider_id.clone(),
        prepared.provider.clone(),
        code_home.clone(),
    );
    let catalog = remote_manager.refresh_remote_models_no_cache().await;
    if let Some(error) = catalog_error(&catalog) {
        if let Err(rollback_error) = rollback_staged_secret(staged).await {
            return Err(format!("{error} {rollback_error}"));
        }
        return Err(error);
    }

    let service = ConfigService::new_with_defaults(code_home, cwd);
    if let Err(error) = persist_provider(&service, &prepared) {
        if let Err(rollback_error) = rollback_staged_secret(staged).await {
            return Err(format!("{error} {rollback_error}"));
        }
        return Err(error);
    }

    Ok(DirectProviderAddOutcome {
        provider_id: prepared.provider_id,
        models: catalog.models,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use code_keyring_store::tests::MockKeyringStore;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn direct_provider_endpoint_no_key_persistence_omits_secret_reference() {
        let code_home = tempdir().expect("code home");
        let cwd = tempdir().expect("cwd");
        let prepared = prepare_direct_provider(DirectProviderRequest {
            display_name: "Local Ollama".to_owned(),
            base_url: "http://127.0.0.1:11434/v1/".to_owned(),
            api_key: None,
            wire_api: WireApi::Chat,
        })
        .expect("prepared provider");
        let service = ConfigService::new_with_defaults(
            code_home.path().to_path_buf(),
            cwd.path().to_path_buf(),
        );

        persist_provider(&service, &prepared).expect("persist provider");

        let contents = std::fs::read_to_string(code_home.path().join("config.toml"))
            .expect("config contents");
        let parsed: toml::Value = toml::from_str(&contents).expect("parse config document");
        let provider = &parsed["model_providers"][prepared.provider_id.as_str()];
        assert_eq!(provider["base_url"].as_str(), Some("http://127.0.0.1:11434/v1"));
        assert!(provider.get("env_key").is_none());
    }

    #[test]
    fn direct_provider_endpoint_encrypted_key_persistence_keeps_only_reference_in_config() {
        let code_home = tempdir().expect("code home");
        let cwd = tempdir().expect("cwd");
        let keyring = Arc::new(MockKeyringStore::default());
        let manager = code_secrets::SecretsManager::new_with_keyring_store(
            code_home.path().to_path_buf(),
            code_secrets::SecretsBackendKind::Local,
            keyring,
        );
        let prepared = prepare_direct_provider(DirectProviderRequest {
            display_name: "Company Gateway".to_owned(),
            base_url: "https://gateway.example/v1".to_owned(),
            api_key: Some("top-secret-provider-key".to_owned()),
            wire_api: WireApi::Responses,
        })
        .expect("prepared provider");
        let service = ConfigService::new_with_defaults(
            code_home.path().to_path_buf(),
            cwd.path().to_path_buf(),
        );

        let staged = stage_secret(manager.clone(), &prepared)
            .expect("stage secret")
            .expect("stored secret");
        persist_provider(&service, &prepared).expect("persist provider");

        let (secret_name, _) = prepared.secret.as_ref().expect("secret reference");
        assert_eq!(
            manager
                .get(&code_secrets::SecretScope::Global, secret_name)
                .expect("read secret")
                .as_deref(),
            Some("top-secret-provider-key")
        );
        let contents = std::fs::read_to_string(code_home.path().join("config.toml"))
            .expect("config contents");
        assert!(contents.contains(secret_name.as_str()));
        assert!(!contents.contains("top-secret-provider-key"));
        let encrypted = std::fs::read(code_home.path().join("secrets/local.age"))
            .expect("encrypted store");
        assert!(!encrypted
            .windows("top-secret-provider-key".len())
            .any(|window| window == b"top-secret-provider-key"));

        staged.rollback().expect("restore secret state");
    }
}
