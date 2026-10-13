//! Message composition helpers and types for the chat widget.

use code_core::protocol::InputItem;
use code_protocol::models::ContentItem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub(crate) struct UserMessage {
    /// What to show in the chat history (keeps placeholders like "[image: name.png]")
    pub(crate) display_text: String,
    /// Items to send to the core/model in the correct order, with inline
    /// markers preceding images so the LLM knows placement.
    pub(crate) ordered_items: Vec<InputItem>,
    /// Skip adding this message to the persisted history when true.
    pub(crate) suppress_persistence: bool,
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        let has_content = !text.trim().is_empty();
        let ordered = if has_content {
            vec![InputItem::Text { text: text.clone() }]
        } else {
            Vec::new()
        };
        Self {
            display_text: text,
            ordered_items: ordered,
            suppress_persistence: false,
        }
    }
}

impl UserMessage {
    /// Resolve local image paths before this message enters the retry/queue state.
    ///
    /// Core request serialization also resolves `LocalImage`, but leaving raw local paths in the
    /// TUI queue means a corrupt image can be retried forever after a failed turn. Converting once
    /// at dispatch time makes retry state stable: valid images become data URLs and invalid images
    /// become text diagnostics instead of re-reading the same bad file.
    pub(crate) fn sanitize_local_images_for_dispatch(&mut self) {
        if !self
            .ordered_items
            .iter()
            .any(|item| matches!(item, InputItem::LocalImage { .. }))
        {
            return;
        }

        let mut image_index = 0;
        let mut sanitized = Vec::with_capacity(self.ordered_items.len());
        for item in self.ordered_items.drain(..) {
            match item {
                InputItem::LocalImage { path } => {
                    image_index += 1;
                    sanitized.extend(
                        code_protocol::models::local_image_content_items_with_label_number(
                            &path,
                            Some(image_index),
                        )
                        .into_iter()
                        .filter_map(content_item_to_input_item),
                    );
                }
                other => sanitized.push(other),
            }
        }
        self.ordered_items = sanitized;
    }
}

fn content_item_to_input_item(item: ContentItem) -> Option<InputItem> {
    match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            Some(InputItem::Text { text })
        }
        ContentItem::InputImage { image_url } => Some(InputItem::Image { image_url }),
    }
}

pub(crate) fn create_initial_user_message(text: String, image_paths: Vec<PathBuf>) -> Option<UserMessage> {
    if text.is_empty() && image_paths.is_empty() {
        None
    } else {
        let has_text = !text.trim().is_empty();
        let mut ordered: Vec<InputItem> = Vec::with_capacity(
            usize::from(has_text) + image_paths.len() * 2,
        );
        if has_text {
            ordered.push(InputItem::Text { text: text.clone() });
        }
        for path in image_paths {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
            ordered.push(InputItem::Text { text: format!("[image: {filename}]") });
            ordered.push(InputItem::LocalImage { path });
        }
        Some(UserMessage {
            display_text: text,
            ordered_items: ordered,
            suppress_persistence: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_local_images_replaces_corrupt_image_with_text() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let corrupt = dir.path().join("corrupt.png");
        std::fs::write(&corrupt, b"not actually a png").expect("write corrupt image");
        let mut message = UserMessage {
            display_text: "see image".to_string(),
            ordered_items: vec![
                code_core::protocol::InputItem::Text {
                    text: "see image".to_string(),
                },
                code_core::protocol::InputItem::LocalImage { path: corrupt },
            ],
            suppress_persistence: false,
        };

        message.sanitize_local_images_for_dispatch();

        assert!(message.ordered_items.iter().all(|item| {
            !matches!(item, code_core::protocol::InputItem::LocalImage { .. })
        }));
        assert!(message.ordered_items.iter().any(|item| matches!(
            item,
            code_core::protocol::InputItem::Text { text }
                if text.contains("corrupt.png") && text.contains("image")
        )));
    }
}
