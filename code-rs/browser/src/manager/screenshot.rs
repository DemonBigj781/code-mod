use crate::BrowserError;
use crate::Result;
use image::GenericImageView;
use std::io::Cursor;
use std::sync::Arc;
use tokio::time::Duration;
use tracing::warn;

use super::BrowserManager;

impl BrowserManager {
    pub async fn capture_screenshot_with_url(
        &self,
    ) -> Result<(Vec<std::path::PathBuf>, Option<String>)> {
        let (images, url) = self.capture_screenshot_refs_internal().await?;
        Ok((
            images
                .into_iter()
                .map(|image| std::path::PathBuf::from(image.path))
                .collect(),
            Some(url),
        ))
    }

    pub async fn capture_screenshot(&self) -> Result<Vec<std::path::PathBuf>> {
        let (images, _) = self.capture_screenshot_refs_internal().await?;
        Ok(images
            .into_iter()
            .map(|image| std::path::PathBuf::from(image.path))
            .collect())
    }

    pub async fn capture_screenshot_mode(
        &self,
        mode: crate::page::ScreenshotMode,
    ) -> Result<(Vec<std::path::PathBuf>, String)> {
        let (images, url) = self.capture_screenshot_refs_with_mode(mode).await?;
        Ok((
            images
                .into_iter()
                .map(|image| std::path::PathBuf::from(image.path))
                .collect(),
            url,
        ))
    }

    pub async fn capture_screenshot_refs_mode(
        &self,
        mode: crate::page::ScreenshotMode,
    ) -> Result<(Vec<crate::assets::ImageRef>, String)> {
        self.capture_screenshot_refs_with_mode(mode).await
    }

    pub async fn capture_screenshot_refs_with_url(
        &self,
    ) -> Result<(Vec<crate::assets::ImageRef>, String)> {
        self.capture_screenshot_refs_internal().await
    }

    async fn capture_screenshot_refs_internal(
        &self,
    ) -> Result<(Vec<crate::assets::ImageRef>, String)> {
        // Always capture from the active page; do not create background tabs.
        self.capture_screenshot_regular().await
    }

    /// Capture a screenshot through the active embedded or external backend.
    async fn capture_screenshot_regular(&self) -> Result<(Vec<crate::assets::ImageRef>, String)> {
        // Determine screenshot mode from config.
        let config = self.config.read().await;
        let mode = if config.fullpage {
            crate::page::ScreenshotMode::FullPage {
                segments_max: Some(config.segments_max),
            }
        } else {
            crate::page::ScreenshotMode::Viewport
        };
        drop(config);

        self.capture_screenshot_refs_with_mode(mode).await
    }

    async fn capture_screenshot_refs_with_mode(
        &self,
        mode: crate::page::ScreenshotMode,
    ) -> Result<(Vec<crate::assets::ImageRef>, String)> {
        if !self.is_external_connection().await {
            let browser = self.embedded_browser().await?;
            let full_page = matches!(mode, crate::page::ScreenshotMode::FullPage { .. });
            let captured = tokio::time::timeout(
                Duration::from_secs(15),
                browser.screenshot(full_page, code_rustwright_mcp::ScreenshotType::Png),
            )
            .await
            .map_err(|_| {
                BrowserError::ScreenshotError(
                    "Embedded Firefox screenshot timed out after 15 seconds".to_owned(),
                )
            })??;

            let mut image = image::load_from_memory(&captured.bytes).map_err(|error| {
                BrowserError::ScreenshotError(format!(
                    "Embedded Firefox returned an invalid screenshot: {error}"
                ))
            })?;
            if let crate::page::ScreenshotMode::Region(region) = mode {
                let max_width = image.width().saturating_sub(region.x);
                let max_height = image.height().saturating_sub(region.y);
                let width = region.width.min(max_width);
                let height = region.height.min(max_height);
                if width == 0 || height == 0 {
                    return Err(BrowserError::ScreenshotError(
                        "Screenshot region is outside the Firefox viewport".to_owned(),
                    ));
                }
                image = image.crop_imm(region.x, region.y, width, height);
            }
            let (width, height) = image.dimensions();
            let mut png = Cursor::new(Vec::new());
            image
                .write_to(&mut png, image::ImageFormat::Png)
                .map_err(|error| {
                    BrowserError::ScreenshotError(format!(
                        "Failed to encode embedded Firefox screenshot: {error}"
                    ))
                })?;

            let mut assets_guard = self.assets.lock().await;
            if assets_guard.is_none() {
                *assets_guard = Some(Arc::new(crate::assets::AssetManager::new().await?));
            }
            let assets = assets_guard.as_ref().cloned().ok_or_else(|| {
                BrowserError::AssetError("assets manager failed to initialize".to_owned())
            })?;
            drop(assets_guard);

            let image_ref = assets
                .store_screenshot(
                    png.get_ref(),
                    crate::config::ImageFormat::Png,
                    width,
                    height,
                    Self::SCREENSHOT_TTL_MS,
                )
                .await?;
            let current_url = browser
                .page_info()
                .await
                .map(|info| info.url)
                .unwrap_or_else(|_| "about:blank".to_owned());
            self.update_activity().await;
            return Ok((vec![image_ref], current_url));
        }

        // External Chrome remains on the existing CDP screenshot path.
        let page = self.get_or_create_page().await?;

        // Viewport correction is handled inside Page::screenshot for all connections.

        // Initialize assets manager if needed.
        let mut assets_guard = self.assets.lock().await;
        if assets_guard.is_none() {
            *assets_guard = Some(Arc::new(crate::assets::AssetManager::new().await?));
        }
        let assets = assets_guard.as_ref().cloned().ok_or_else(|| {
            BrowserError::AssetError("assets manager failed to initialize".to_owned())
        })?;
        drop(assets_guard);

        // Get current URL with timeout.
        let current_url = if let Ok(Ok(url)) = tokio::time::timeout(Duration::from_secs(3), page.get_current_url())
            .await { url } else {
            warn!("Failed to get current URL, using default");
            "about:blank".to_owned()
        };

        // Capture screenshots with timeout.
        let screenshot_result = tokio::time::timeout(
            Duration::from_secs(15), // Allow up to 15 seconds for screenshot.
            page.screenshot(mode),
        )
        .await;

        let screenshots = match screenshot_result {
            Ok(Ok(shots)) => shots,
            Ok(Err(e)) => {
                return Err(BrowserError::ScreenshotError(format!(
                    "Screenshot capture failed: {e}"
                )));
            }
            Err(_) => {
                return Err(BrowserError::ScreenshotError(
                    "Screenshot capture timed out after 15 seconds".to_owned(),
                ));
            }
        };

        // Store screenshots and get paths.
        let mut images = Vec::new();
        for screenshot in screenshots {
            let image_ref = assets
                .store_screenshot(
                    &screenshot.data,
                    screenshot.format,
                    screenshot.width,
                    screenshot.height,
                    Self::SCREENSHOT_TTL_MS,
                )
                .await?;
            images.push(image_ref);
        }

        self.update_activity().await;
        Ok((images, current_url))
    }
}
