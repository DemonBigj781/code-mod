use crate::Result;
use crate::assets::AssetManager;
use crate::assets::ImageRef;
use crate::manager::BrowserManager;
use std::sync::Arc;
use tracing::debug;
use tracing::info;

pub struct BrowserInjector {
    manager: Arc<BrowserManager>,
    asset_manager: Arc<AssetManager>,
}

impl BrowserInjector {
    pub fn new(manager: Arc<BrowserManager>, asset_manager: Arc<AssetManager>) -> Self {
        Self {
            manager,
            asset_manager,
        }
    }

    pub async fn inject_pre_llm_call(&self) -> Result<Option<InjectionResult>> {
        if !self.manager.is_enabled().await {
            return Ok(None);
        }

        if let Some((elapsed, timeout)) = self.manager.idle_elapsed_past_timeout().await {
            info!(
                "Skipping pre-LLM screenshot; browser idle for {:?} (timeout {:?})",
                elapsed,
                timeout
            );
            return Ok(None);
        }

        debug!("Browser enabled, capturing pre-LLM screenshot");

        let config = self.manager.get_config().await;
        let (images, current_url) = self.manager.capture_screenshot_refs_with_url().await?;

        let system_hint = format!(
            "A fresh screenshot of the active page ({current_url}) is attached; use browser_* tools to navigate or capture more."
        );

        let segments_captured = images.len();

        Ok(Some(InjectionResult {
            images,
            system_hint,
            metadata: InjectionMetadata {
                url: current_url,
                fullpage: config.fullpage,
                segments_captured,
            },
        }))
    }

    pub async fn cleanup_expired(&self) -> Result<()> {
        self.asset_manager.cleanup_expired().await
    }
}

#[derive(Debug)]
pub struct InjectionResult {
    pub images: Vec<ImageRef>,
    pub system_hint: String,
    pub metadata: InjectionMetadata,
}

#[derive(Debug)]
pub struct InjectionMetadata {
    pub url: String,
    pub fullpage: bool,
    pub segments_captured: usize,
}
