use std::sync::atomic::{AtomicI64, Ordering};

use rmcp::model::RequestId;
use serde::Deserialize;
use serde_json::Value;

use crate::actor::{
    BrowserActor, EmbeddedBrowserOp, EmbeddedBrowserOutput, EmbeddedTab,
};
use crate::actor::{BrowserError, BrowserOp, BrowserOutput, ScreenshotType};

/// In-process Camoufox/Juggler browser used by Code's built-in browser surface.
pub struct EmbeddedBrowser {
    actor: BrowserActor,
    next_request_id: AtomicI64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EmbeddedPageInfo {
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "readyState")]
    pub ready_state: String,
}

#[derive(Debug)]
pub struct EmbeddedScreenshot {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub extension: &'static str,
}

impl EmbeddedBrowser {
    pub fn new() -> Self {
        Self {
            actor: BrowserActor::spawn(),
            next_request_id: AtomicI64::new(1),
        }
    }

    pub async fn navigate(&self, url: &str) -> Result<EmbeddedPageInfo, BrowserError> {
        let _ = self.execute(BrowserOp::Navigate(url.to_owned())).await?;
        self.page_info().await
    }

    pub async fn page_info(&self) -> Result<EmbeddedPageInfo, BrowserError> {
        let output = self.execute_embedded(EmbeddedBrowserOp::PageInfo).await?;
        let EmbeddedBrowserOutput::Value(value) = output else {
            return Err(BrowserError::Stopped);
        };
        serde_json::from_value(value)
            .map_err(|error| BrowserError::Message(format!("invalid page info: {error}")))
    }

    pub async fn resize(&self, width: u32, height: u32) -> Result<(), BrowserError> {
        let _ = self.execute(BrowserOp::Resize { width, height }).await?;
        Ok(())
    }

    pub async fn evaluate(&self, expression: &str) -> Result<Value, BrowserError> {
        let output = self
            .execute(BrowserOp::EvaluateWire {
                expression: expression.to_owned(),
            })
            .await?;
        let text = output_text(output)?;
        serde_json::from_str(&text)
            .map_err(|error| BrowserError::Message(format!("invalid evaluation result: {error}")))
    }

    pub async fn console_messages(&self, lines: Option<usize>) -> Result<Value, BrowserError> {
        let output = self
            .execute(BrowserOp::ConsoleMessages {
                level: rustwright_agent::ConsoleLevel::Debug,
                all: true,
                filename: None,
            })
            .await?;
        let text = output_text(output)?;
        let mut value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        if let (Some(lines), Some(records)) = (lines, value.as_array_mut())
            && records.len() > lines
        {
            records.drain(..records.len() - lines);
        }
        Ok(value)
    }

    pub async fn screenshot(
        &self,
        full_page: bool,
        image_type: ScreenshotType,
    ) -> Result<EmbeddedScreenshot, BrowserError> {
        let output = self
            .execute(BrowserOp::TakeScreenshot {
                full_page,
                image_type,
            })
            .await?;
        match output {
            BrowserOutput::Image {
                bytes,
                mime,
                extension,
            } => Ok(EmbeddedScreenshot {
                bytes,
                mime,
                extension,
            }),
            _ => Err(BrowserError::Stopped),
        }
    }

    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::MouseMove { x, y }).await
    }

    pub async fn mouse_down(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::MouseDown { x, y }).await
    }

    pub async fn mouse_up(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::MouseUp { x, y }).await
    }

    pub async fn click(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::Click { x, y }).await
    }

    pub async fn type_text(&self, text: &str) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::TypeText(text.to_owned()))
            .await
    }

    pub async fn press_key(&self, key: &str) -> Result<(), BrowserError> {
        let _ = self
            .execute(BrowserOp::PressKey {
                target: None,
                key: key.to_owned(),
            })
            .await?;
        Ok(())
    }

    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::Scroll { dx, dy }).await
    }

    pub async fn history_back(&self) -> Result<(), BrowserError> {
        let _ = self.execute(BrowserOp::NavigateBack).await?;
        Ok(())
    }

    pub async fn history_forward(&self) -> Result<(), BrowserError> {
        let _ = self.execute(BrowserOp::NavigateForward).await?;
        Ok(())
    }

    pub async fn tabs(&self) -> Result<Vec<EmbeddedTab>, BrowserError> {
        match self.execute_embedded(EmbeddedBrowserOp::Tabs).await? {
            EmbeddedBrowserOutput::Tabs(tabs) => Ok(tabs),
            _ => Err(BrowserError::Stopped),
        }
    }

    pub async fn new_tab(&self, url: &str) -> Result<String, BrowserError> {
        match self
            .execute_embedded(EmbeddedBrowserOp::NewTab(url.to_owned()))
            .await?
        {
            EmbeddedBrowserOutput::TargetId(target_id) => Ok(target_id),
            _ => Err(BrowserError::Stopped),
        }
    }

    pub async fn select_target(&self, target_id: &str) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::SelectTarget(target_id.to_owned()))
            .await
    }

    pub async fn close_target(&self, target_id: &str) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::CloseTarget(target_id.to_owned()))
            .await
    }

    pub async fn cookies(&self) -> Result<Vec<Value>, BrowserError> {
        match self.execute_embedded(EmbeddedBrowserOp::Cookies).await? {
            EmbeddedBrowserOutput::Value(Value::Array(cookies)) => Ok(cookies),
            _ => Err(BrowserError::Stopped),
        }
    }

    pub async fn set_cookies(&self, cookies: Vec<Value>) -> Result<(), BrowserError> {
        self.execute_none(EmbeddedBrowserOp::SetCookies(cookies)).await
    }

    pub async fn close(&self) -> Result<(), BrowserError> {
        let _ = self.execute(BrowserOp::Close).await?;
        Ok(())
    }

    async fn execute(&self, op: BrowserOp) -> Result<BrowserOutput, BrowserError> {
        self.actor.execute(self.request_id(), op).await
    }

    async fn execute_embedded(
        &self,
        op: EmbeddedBrowserOp,
    ) -> Result<EmbeddedBrowserOutput, BrowserError> {
        self.actor.execute_embedded(self.request_id(), op).await
    }

    async fn execute_none(&self, op: EmbeddedBrowserOp) -> Result<(), BrowserError> {
        match self.execute_embedded(op).await? {
            EmbeddedBrowserOutput::None => Ok(()),
            _ => Err(BrowserError::Stopped),
        }
    }

    fn request_id(&self) -> RequestId {
        RequestId::Number(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for EmbeddedBrowser {
    fn default() -> Self {
        Self::new()
    }
}

fn output_text(output: BrowserOutput) -> Result<String, BrowserError> {
    match output {
        BrowserOutput::Text(text) | BrowserOutput::ShapedText { text, .. } => Ok(text),
        BrowserOutput::Image { .. } => Err(BrowserError::Stopped),
    }
}
