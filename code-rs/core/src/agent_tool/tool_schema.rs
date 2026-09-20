use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::openai_tools::JsonSchema;
use crate::openai_tools::OpenAiTool;
use crate::openai_tools::ResponsesApiTool;

pub fn create_agent_tool(_allowed_models: &[String]) -> OpenAiTool {
    let mut properties = BTreeMap::new();

    properties.insert(
        "action".to_owned(),
        JsonSchema::String {
            description: Some(
                "Required: choose one of ['create','status','wait','result','cancel','list']".to_owned(),
            ),
            allowed_values: Some(
                ["create", "status", "wait", "result", "cancel", "list"]
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
        },
    );

    let mut create_properties = BTreeMap::new();
    create_properties.insert(
        "name".to_owned(),
        JsonSchema::String {
            description: Some(
                "Display name shown in the UI (e.g., \"Plan TUI Refactor\")".to_owned(),
            ),
            allowed_values: None,
        },
    );
    create_properties.insert(
        "task".to_owned(),
        JsonSchema::String {
            description: Some("Task prompt to execute".to_owned()),
            allowed_values: None,
        },
    );
    create_properties.insert(
        "context".to_owned(),
        JsonSchema::String {
            description: Some("Optional background context".to_owned()),
            allowed_values: None,
        },
    );
    create_properties.insert(
        "files".to_owned(),
        JsonSchema::Array {
            items: Box::new(JsonSchema::String {
                description: None,
                allowed_values: None,
            }),
            description: Some("Optional array of file paths to include in context".to_owned()),
        },
    );
    create_properties.insert(
        "output".to_owned(),
        JsonSchema::String {
            description: Some("Optional desired output description".to_owned()),
            allowed_values: None,
        },
    );
    properties.insert(
        "create".to_owned(),
        JsonSchema::Object {
            properties: create_properties,
            required: Some(vec!["task".to_owned()]),
            additional_properties: Some(false.into()),
        },
    );

    let mut status_properties = BTreeMap::new();
    status_properties.insert(
        "agent_id".to_owned(),
        JsonSchema::String {
            description: Some("Agent identifier to inspect".to_owned()),
            allowed_values: None,
        },
    );
    properties.insert(
        "status".to_owned(),
        JsonSchema::Object {
            properties: status_properties,
            required: Some(vec!["agent_id".to_owned()]),
            additional_properties: Some(false.into()),
        },
    );

    let mut result_properties = BTreeMap::new();
    result_properties.insert(
        "agent_id".to_owned(),
        JsonSchema::String {
            description: Some("Agent identifier whose result should be fetched".to_owned()),
            allowed_values: None,
        },
    );
    properties.insert(
        "result".to_owned(),
        JsonSchema::Object {
            properties: result_properties,
            required: Some(vec!["agent_id".to_owned()]),
            additional_properties: Some(false.into()),
        },
    );

    let mut cancel_properties = BTreeMap::new();
    cancel_properties.insert(
        "agent_id".to_owned(),
        JsonSchema::String {
            description: Some("Cancel a specific agent".to_owned()),
            allowed_values: None,
        },
    );
    cancel_properties.insert(
        "batch_id".to_owned(),
        JsonSchema::String {
            description: Some("Cancel all agents in the batch".to_owned()),
            allowed_values: None,
        },
    );
    properties.insert(
        "cancel".to_owned(),
        JsonSchema::Object {
            properties: cancel_properties,
            required: Some(Vec::new()),
            additional_properties: Some(false.into()),
        },
    );

    let mut wait_properties = BTreeMap::new();
    wait_properties.insert(
        "agent_id".to_owned(),
        JsonSchema::String {
            description: Some("Wait for a specific agent".to_owned()),
            allowed_values: None,
        },
    );
    wait_properties.insert(
        "batch_id".to_owned(),
        JsonSchema::String {
            description: Some("Wait for any agent in the batch".to_owned()),
            allowed_values: None,
        },
    );
    wait_properties.insert(
        "timeout_seconds".to_owned(),
        JsonSchema::Number {
            description: Some("Optional timeout before giving up (default 300, max 600)".to_owned()),
        },
    );
    wait_properties.insert(
        "return_all".to_owned(),
        JsonSchema::Boolean {
            description: Some(
                "When waiting on a batch, return all completed agents instead of the first".to_owned(),
            ),
        },
    );
    properties.insert(
        "wait".to_owned(),
        JsonSchema::Object {
            properties: wait_properties,
            required: Some(Vec::new()),
            additional_properties: Some(false.into()),
        },
    );

    let mut list_properties = BTreeMap::new();
    list_properties.insert(
        "status_filter".to_owned(),
        JsonSchema::String {
            description: Some(
                "Optional status filter (pending, running, completed, failed, cancelled)".to_owned(),
            ),
            allowed_values: None,
        },
    );
    list_properties.insert(
        "batch_id".to_owned(),
        JsonSchema::String {
            description: Some("Limit results to a batch".to_owned()),
            allowed_values: None,
        },
    );
    list_properties.insert(
        "recent_only".to_owned(),
        JsonSchema::Boolean {
            description: Some("When true, only include agents from the last two hours".to_owned()),
        },
    );
    properties.insert(
        "list".to_owned(),
        JsonSchema::Object {
            properties: list_properties,
            required: Some(Vec::new()),
            additional_properties: Some(false.into()),
        },
    );

    let required = Some(vec!["action".to_owned()]);

    OpenAiTool::Function(ResponsesApiTool {
        name: "agent".to_owned(),
        description:
            "Unified agent manager for launching, monitoring, and collecting results from asynchronous agents.".to_owned(),
        strict: false,
        parameters: JsonSchema::Object {
            properties,
            required,
            additional_properties: Some(false.into()),
        },
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAgentParams {
    pub task: String,
    pub context: Option<String>,
    pub output: Option<String>,
    pub files: Option<Vec<String>>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCreateOptions {
    pub task: Option<String>,
    pub context: Option<String>,
    pub output: Option<String>,
    pub files: Option<Vec<String>>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentifierOptions {
    pub agent_id: Option<String>,
    pub batch_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCancelOptions {
    pub agent_id: Option<String>,
    pub batch_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWaitOptions {
    pub agent_id: Option<String>,
    pub batch_id: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub return_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentListOptions {
    pub status_filter: Option<String>,
    pub batch_id: Option<String>,
    pub recent_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolRequest {
    pub action: String,
    pub create: Option<AgentCreateOptions>,
    pub status: Option<AgentIdentifierOptions>,
    pub result: Option<AgentIdentifierOptions>,
    pub cancel: Option<AgentCancelOptions>,
    pub wait: Option<AgentWaitOptions>,
    pub list: Option<AgentListOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckAgentStatusParams {
    pub agent_id: String,
    pub batch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAgentResultParams {
    pub agent_id: String,
    pub batch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelAgentParams {
    pub agent_id: Option<String>,
    pub batch_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitForAgentParams {
    pub agent_id: Option<String>,
    pub batch_id: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub return_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsParams {
    pub status_filter: Option<String>,
    pub batch_id: Option<String>,
    pub recent_only: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_schema_leaves_agent_selection_and_permissions_to_settings() {
        let schema = serde_json::to_value(create_agent_tool(&["configured-model".to_owned()]))
            .expect("agent tool should serialize");
        let create_properties = schema
            .pointer("/parameters/properties/create/properties")
            .and_then(serde_json::Value::as_object)
            .expect("create properties should be an object");

        assert!(!create_properties.contains_key("models"));
        assert!(!create_properties.contains_key("write"));
        assert!(!create_properties.contains_key("read_only"));
    }

    #[test]
    fn create_options_reject_agent_selection_and_permission_overrides() {
        for forbidden in [
            json!({"task": "Inspect the configured project state", "models": ["other"]}),
            json!({"task": "Inspect the configured project state", "write": true}),
            json!({"task": "Inspect the configured project state", "read_only": false}),
        ] {
            assert!(
                serde_json::from_value::<AgentCreateOptions>(forbidden).is_err(),
                "agent creation must reject Settings-owned fields"
            );
        }
    }
}
