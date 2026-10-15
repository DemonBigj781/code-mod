use super::{JsonSchema, OpenAiTool, ResponsesApiTool};
use std::collections::BTreeMap;

pub(super) fn create_session_title_tool() -> OpenAiTool {
    OpenAiTool::Function(ResponsesApiTool {
        name: "set_session_title".to_owned(),
        description: "Name this conversation using a short, relevant title (usually 2-6 words). Call this when the topic first becomes clear and again when the discussion materially changes topic. Choose the title yourself from the current discussion; do not copy the user's first message. Avoid secrets and personal data. Only this session can be renamed; the title is saved across restarts. Keep the existing title for minor follow-ups.".to_owned(),
        strict: false,
        parameters: JsonSchema::Object {
            properties: BTreeMap::from([("title".to_owned(), JsonSchema::String {
                description: Some("Short current-topic title, 1-80 characters, single line.".to_owned()),
                allowed_values: None,
            })]),
            required: Some(vec!["title".to_owned()]),
            additional_properties: Some(false.into()),
        },
    })
}
