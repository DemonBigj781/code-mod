use crate::Result;
use crate::config::BrowserConfig;
use tokio::time::Duration;

use super::BrowserManager;
use super::BrowserStatus;

impl BrowserManager {
    pub async fn is_enabled(&self) -> bool {
        self.config.read().await.enabled
    }

    pub fn is_enabled_sync(&self) -> bool {
        self.config.try_read().is_ok_and(|c| c.enabled)
    }

    /// Returns how long the browser has been idle when it exceeds the configured timeout.
    /// Used to decide whether we should avoid taking fresh screenshots (which would reset
    /// the idle timer) until the user interacts with browser_* tools again.
    pub async fn idle_elapsed_past_timeout(&self) -> Option<(Duration, Duration)> {
        let idle_timeout = Duration::from_millis(self.config.read().await.idle_timeout_ms);
        let last = *self.last_activity.lock().await;
        let elapsed = last.elapsed();
        (elapsed > idle_timeout).then_some((elapsed, idle_timeout))
    }

    /// Get a description of the browser connection type
    pub async fn get_browser_type(&self) -> String {
        let config = self.config.read().await;
        if config.connect_ws.is_some() || config.connect_port.is_some() {
            "CDP-connected to user's Chrome browser".to_owned()
        } else {
            "internal headless Firefox browser (Camoufox/Juggler)".to_owned()
        }
    }

    pub async fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.config.write().await.enabled = enabled;

        if enabled {
            self.start().await?;
        } else {
            self.stop().await?;
        }

        Ok(())
    }

    /// Remove the persistent inspection highlight without exposing protocol details.
    pub async fn clear_inspection_highlight(&self) -> Result<()> {
        if !self.is_external_connection().await {
            let script = "document.getElementById('__code_inspect_highlight')?.remove(); true";
            let _ = self.embedded_browser().await?.evaluate(script).await?;
            return Ok(());
        }
        let _ = self
            .execute_cdp("Overlay.hideHighlight", serde_json::json!({}))
            .await?;
        Ok(())
    }

    pub async fn get_cookies(&self, urls: Option<Vec<String>>) -> Result<serde_json::Value> {
        if !self.is_external_connection().await {
            let cookies = self.embedded_browser().await?.cookies().await?;
            self.update_activity().await;
            return Ok(serde_json::json!({ "cookies": cookies }));
        }
        let _ = self.execute_cdp("Network.enable", serde_json::json!({})).await;
        match urls {
            Some(urls) if !urls.is_empty() => {
                self.execute_cdp("Network.getCookies", serde_json::json!({ "urls": urls }))
                    .await
            }
            _ => self
                .execute_cdp("Network.getAllCookies", serde_json::json!({}))
                .await,
        }
    }

    pub async fn set_cookies(&self, cookies: Vec<serde_json::Value>) -> Result<serde_json::Value> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.set_cookies(cookies).await?;
            self.update_activity().await;
            return Ok(serde_json::json!({}));
        }
        let _ = self.execute_cdp("Network.enable", serde_json::json!({})).await;
        self.execute_cdp("Network.setCookies", serde_json::json!({ "cookies": cookies }))
            .await
    }

    pub async fn update_config(&self, updates: impl FnOnce(&mut BrowserConfig)) -> Result<()> {
        let mut config = self.config.write().await;
        updates(&mut config);

        if let Some(page) = self.page.lock().await.as_ref() {
            // Avoid viewport manipulation for external CDP connections to prevent focus/flicker
            let is_external = config.connect_port.is_some() || config.connect_ws.is_some();
            if !is_external {
                page.update_viewport(config.viewport.clone())?;
            }
        }
        if config.connect_port.is_none()
            && config.connect_ws.is_none()
            && let Some(browser) = self.embedded_browser.lock().await.clone()
        {
            browser
                .resize(config.viewport.width, config.viewport.height)
                .await?;
        }

        Ok(())
    }

    pub async fn get_config(&self) -> BrowserConfig {
        self.config.read().await.clone()
    }

    pub async fn get_current_url(&self) -> Option<String> {
        if !self.is_external_connection().await {
            let browser = self.embedded_browser.lock().await.clone()?;
            return browser.page_info().await.ok().map(|info| info.url);
        }
        let page_guard = self.page.lock().await;
        if let Some(page) = page_guard.as_ref() {
            page.get_current_url().await.ok()
        } else {
            None
        }
    }

    pub async fn get_title(&self) -> Option<String> {
        if !self.is_external_connection().await {
            let browser = self.embedded_browser.lock().await.clone()?;
            return browser.page_info().await.ok().map(|info| info.title);
        }
        let page_guard = self.page.lock().await;
        if let Some(page) = page_guard.as_ref() {
            page.get_title().await
        } else {
            None
        }
    }

    pub async fn get_status(&self) -> BrowserStatus {
        let config = self.config.read().await;
        let browser_active = self.browser.lock().await.is_some()
            || self.embedded_browser.lock().await.is_some();
        let current_url = self.get_current_url().await;

        BrowserStatus {
            enabled: config.enabled,
            browser_active,
            current_url,
            viewport: config.viewport.clone(),
            fullpage: config.fullpage,
        }
    }

    pub fn set_enabled_sync(&self, enabled: bool) {
        // Try to set immediately if possible, otherwise spawn a task
        if let Ok(mut cfg) = self.config.try_write() {
            cfg.enabled = enabled;
        } else {
            let config = self.config.clone();
            tokio::spawn(async move {
                let mut cfg = config.write().await;
                cfg.enabled = enabled;
            });
        }
    }

    pub fn set_fullpage_sync(&self, fullpage: bool) {
        if let Ok(mut cfg) = self.config.try_write() {
            cfg.fullpage = fullpage;
        } else {
            let config = self.config.clone();
            tokio::spawn(async move {
                let mut cfg = config.write().await;
                cfg.fullpage = fullpage;
            });
        }
    }

    pub fn set_viewport_sync(&self, width: u32, height: u32) {
        if let Ok(mut cfg) = self.config.try_write() {
            cfg.viewport.width = width;
            cfg.viewport.height = height;
        } else {
            let config = self.config.clone();
            tokio::spawn(async move {
                let mut cfg = config.write().await;
                cfg.viewport.width = width;
                cfg.viewport.height = height;
            });
        }
    }

    pub fn set_segments_max_sync(&self, segments_max: usize) {
        if let Ok(mut cfg) = self.config.try_write() {
            cfg.segments_max = segments_max;
        } else {
            let config = self.config.clone();
            tokio::spawn(async move {
                let mut cfg = config.write().await;
                cfg.segments_max = segments_max;
            });
        }
    }

    pub fn get_status_sync(&self) -> String {
        // Use try operations to avoid blocking - return cached/default values if locks are held
        let (enabled, vw, vh, fullpage, is_external) = self
            .config
            .try_read()
            .map(|c| {
                let is_ext = c.connect_port.is_some() || c.connect_ws.is_some();
                (c.enabled, c.viewport.width, c.viewport.height, c.fullpage, is_ext)
            })
            .unwrap_or((false, 1024, 768, false, false));

        let browser_active = self
            .browser
            .try_lock()
            .is_ok_and(|b| b.is_some())
            || self
                .embedded_browser
                .try_lock()
                .is_ok_and(|browser| browser.is_some());

        let mode = if enabled { "enabled" } else { "disabled" };
        let fp = if fullpage { "on" } else { "off" };
        let conn_type = if is_external {
            "external Chrome (CDP via /chrome)"
        } else {
            "built-in headless Firefox (Camoufox/Juggler via /browser)"
        };

        let mut status = format!(
            "Browser status:\n• Mode: {mode}\n• Type: {conn_type}\n• Viewport: {vw}×{vh}\n• Full-page: {fp}",
        );

        if browser_active {
            status.push_str("\n• Connection: active");
        } else {
            status.push_str("\n• Connection: inactive");
        }

        status
    }
}
