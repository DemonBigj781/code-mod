use crate::Result;

use super::BrowserManager;

impl BrowserManager {
    /// Move the mouse to the specified coordinates.
    pub async fn move_mouse(&self, x: f64, y: f64) -> Result<()> {
        if !self.is_external_connection().await {
            let browser = self.embedded_browser().await?;
            browser.mouse_move(x, y).await?;
            *self.cursor_position.lock().await = (x, y);
            let script = format!(
                "(() => {{ let cursor = document.getElementById('__code_virtual_cursor'); if (!cursor) {{ cursor = document.createElement('div'); cursor.id = '__code_virtual_cursor'; Object.assign(cursor.style, {{position:'fixed',zIndex:'2147483647',width:'0',height:'0',pointerEvents:'none',borderTop:'10px solid transparent',borderBottom:'10px solid transparent',borderLeft:'16px solid #16a8ff',filter:'drop-shadow(0 0 1px #fff)'}}); document.documentElement.appendChild(cursor); }} cursor.style.left = '{}px'; cursor.style.top = '{}px'; return true; }})()",
                x, y
            );
            let _ = browser.evaluate(&script).await;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.move_mouse(x, y).await
    }

    /// Move the mouse by relative offset from current position.
    pub async fn move_mouse_relative(&self, dx: f64, dy: f64) -> Result<(f64, f64)> {
        if !self.is_external_connection().await {
            let (x, y) = *self.cursor_position.lock().await;
            let next = (x + dx, y + dy);
            self.move_mouse(next.0, next.1).await?;
            return Ok(next);
        }
        let page = self.get_or_create_page().await?;
        page.move_mouse_relative(dx, dy).await
    }

    /// Click at the specified coordinates.
    pub async fn click(&self, x: f64, y: f64) -> Result<()> {
        if !self.is_external_connection().await {
            self.move_mouse(x, y).await?;
            self.embedded_browser().await?.click(x, y).await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.click(x, y).await
    }

    /// Click at the current mouse position.
    pub async fn click_at_current(&self) -> Result<(f64, f64)> {
        if !self.is_external_connection().await {
            let (x, y) = *self.cursor_position.lock().await;
            self.embedded_browser().await?.click(x, y).await?;
            self.update_activity().await;
            return Ok((x, y));
        }
        let page = self.get_or_create_page().await?;
        page.click_at_current().await
    }

    /// Perform mouse down at the current position.
    pub async fn mouse_down_at_current(&self) -> Result<(f64, f64)> {
        if !self.is_external_connection().await {
            let (x, y) = *self.cursor_position.lock().await;
            self.embedded_browser().await?.mouse_down(x, y).await?;
            self.update_activity().await;
            return Ok((x, y));
        }
        let page = self.get_or_create_page().await?;
        page.mouse_down_at_current().await
    }

    /// Perform mouse up at the current position.
    pub async fn mouse_up_at_current(&self) -> Result<(f64, f64)> {
        if !self.is_external_connection().await {
            let (x, y) = *self.cursor_position.lock().await;
            self.embedded_browser().await?.mouse_up(x, y).await?;
            self.update_activity().await;
            return Ok((x, y));
        }
        let page = self.get_or_create_page().await?;
        page.mouse_up_at_current().await
    }

    /// Type text into the currently focused element.
    pub async fn type_text(&self, text: &str) -> Result<()> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.type_text(text).await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.type_text(text).await
    }

    /// Press a key (e.g., "Enter", "Tab", "Escape", "`ArrowDown`").
    pub async fn press_key(&self, key: &str) -> Result<()> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.press_key(key).await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.press_key(key).await
    }

    /// Scroll the page by the given delta in pixels.
    pub async fn scroll_by(&self, dx: f64, dy: f64) -> Result<()> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.scroll(dx, dy).await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.scroll_by(dx, dy).await
    }

    /// Navigate browser history backward one entry.
    pub async fn history_back(&self) -> Result<()> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.history_back().await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.go_back().await
    }

    /// Navigate browser history forward one entry.
    pub async fn history_forward(&self) -> Result<()> {
        if !self.is_external_connection().await {
            self.embedded_browser().await?.history_forward().await?;
            self.update_activity().await;
            return Ok(());
        }
        let page = self.get_or_create_page().await?;
        page.go_forward().await
    }

    /// Get the current cursor position.
    pub async fn get_cursor_position(&self) -> Result<(f64, f64)> {
        if !self.is_external_connection().await {
            return Ok(*self.cursor_position.lock().await);
        }
        let page = self.get_or_create_page().await?;
        page.get_cursor_position().await
    }
}
