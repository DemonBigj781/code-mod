//! Fast checks of the actual production schema sources, before the full APE build.

#[path = "../../../code-rs/core/src/openai_tools/json_schema.rs"]
mod json_schema;
#[path = "../../../code-rs/core/src/openai_tools/types.rs"]
mod tool_types;

mod openai_tools {
    pub(crate) use crate::json_schema::JsonSchema;
    pub(crate) use crate::tool_types::{OpenAiTool, ResponsesApiTool};
}

#[path = "../../../code-rs/core/src/agent_tool/tool_schema.rs"]
mod tool_schema;
