use crate::codex::Session;
use crate::codex::ToolCallCtx;
use crate::protocol::EventMsg;
use crate::protocol::ViewImageToolCallEvent;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::events::execute_custom_tool;
use crate::tools::handlers::{tool_error, tool_output};
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use async_trait::async_trait;
use code_protocol::models::ContentItem;
use code_protocol::models::ResponseInputItem;
use std::path::Path;

pub(crate) struct ImageViewToolHandler;

#[async_trait]
impl ToolHandler for ImageViewToolHandler {
    async fn handle(
        &self,
        sess: &Session,
        _turn_diff_tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(inv.ctx.call_id, "image_view expects function-call arguments");
        };

        handle_image_view(sess, &inv.ctx, arguments).await
    }
}

async fn handle_image_view(sess: &Session, ctx: &ToolCallCtx, arguments: String) -> ResponseInputItem {
    use serde::Deserialize;
    use serde_json::Value;
    use std::path::PathBuf;

    #[derive(Deserialize)]
    struct Params {
        path: String,
        #[serde(default)]
        alt_text: Option<String>,
    }

    let mut params_for_event = serde_json::from_str::<Value>(&arguments).ok();
    let parsed: Params = match serde_json::from_str(&arguments) {
        Ok(p) => p,
        Err(e) => {
            return tool_error(
                ctx.call_id.clone(),
                format!("Invalid image_view arguments: {e}"),
            );
        }
    };

    execute_custom_tool(
        sess,
        ctx,
        "image_view".to_owned(),
        params_for_event.take(),
        move || async move {
            let call_id = ctx.call_id.clone();
            let path_str = parsed.path.trim();
            if path_str.is_empty() {
                return tool_error(call_id, "image_view requires a non-empty path");
            }

            let mut resolved = PathBuf::from(path_str);
            if resolved.is_relative() {
                resolved = sess.get_cwd().join(&resolved);
            }
            if let Ok(canon) = resolved.canonicalize() {
                resolved = canon;
            }
            let metadata = match std::fs::metadata(&resolved) {
                Ok(meta) => meta,
                Err(err) => {
                    return tool_error(
                        call_id,
                        format!("image_view could not read {}: {err}", resolved.display()),
                    );
                }
            };
            if !metadata.is_file() {
                return tool_error(
                    call_id,
                    format!("image_view requires a file path, got {}", resolved.display()),
                );
            }

            let image = match prepare_image_for_model(&resolved) {
                Ok(image) => image,
                Err(err) => {
                    return tool_error(
                        call_id,
                        format!("image_view could not process {}: {err}", resolved.display()),
                    );
                }
            };
            let filename = resolved
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image");
            let label = parsed
                .alt_text
                .as_ref()
                .map(|text| text.trim())
                .filter(|text| !text.is_empty())
                .unwrap_or(filename);
            let marker = format!("[image: {label}]");
            let image_message = ResponseInputItem::Message {
                role: "user".to_owned(),
                content: vec![
                    ContentItem::InputText { text: marker },
                    ContentItem::InputImage {
                        image_url: image.into_data_url(),
                    },
                ],
            };
            sess.add_pending_input(image_message);

            sess.send_ordered_from_ctx(
                ctx,
                EventMsg::ViewImageToolCall(ViewImageToolCallEvent {
                    call_id: ctx.call_id.clone(),
                    path: resolved.clone(),
                }),
            )
            .await;

            tool_output(call_id, format!("attached image: {}", resolved.display()))
        },
    )
    .await
}

fn prepare_image_for_model(
    path: &Path,
) -> Result<code_utils_image::EncodedImage, code_utils_image::error::ImageProcessingError> {
    code_utils_image::load_and_resize_to_fit(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ColorType;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    #[test]
    fn image_view_downscales_before_attaching_to_model_input() {
        let temp = tempfile::NamedTempFile::new().expect("temp image");
        let source = image::GrayImage::from_pixel(8_000, 4_000, image::Luma([144u8]));
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(source.as_raw(), 8_000, 4_000, ColorType::L8.into())
            .expect("encode oversized fixture");
        std::fs::write(temp.path(), png).expect("write oversized fixture");

        let processed = prepare_image_for_model(temp.path()).expect("prepare image");
        let patches = code_utils_image::provider_patch_count(processed.width, processed.height);

        assert!(patches < code_utils_image::MAX_PROVIDER_PATCHES);
        assert!(processed.width < 8_000);
        assert!(processed.height < 4_000);
    }
}
