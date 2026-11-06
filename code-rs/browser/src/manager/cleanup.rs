use crate::Result;

use super::BrowserManager;

impl BrowserManager {
    /// Clean up injected artifacts and restore viewport/state where possible.
    /// This does not close the browser; it is safe to call when connected.
    pub async fn cleanup(&self) -> Result<()> {
        // Hide any overlay highlight.
        let _ = self.clear_inspection_highlight().await;

        // Reset device metrics override (best-effort).
        let _ = self
            .execute_cdp("Emulation.clearDeviceMetricsOverride", serde_json::json!({}))
            .await;

        // Remove virtual cursor and related overlays if present.
        let cleanup_js = "
            (function(){
                try { if (window.__vc && typeof window.__vc.destroy === 'function') window.__vc.destroy(); } catch(_) {}
                try { document.getElementById('__code_virtual_cursor')?.remove(); } catch(_) {}
                try { document.getElementById('__code_inspect_highlight')?.remove(); } catch(_) {}
                try { if (window.__code_console_logs) delete window.__code_console_logs; } catch(_) {}
                return true;
            })()
        ";
        let _ = self.execute_javascript(cleanup_js).await;
        Ok(())
    }
}
