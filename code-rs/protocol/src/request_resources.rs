use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ResourceGrantScope {
    #[default]
    NextCommand,
    Session,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequestProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u64>,
}

impl ResourceRequestProfile {
    pub fn is_empty(&self) -> bool {
        self.memory_max_mb.is_none() && self.pids_max.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestResourcesArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub resources: ResourceRequestProfile,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestResourcesResponse {
    /// Values approved by the user before host-level clamping.
    pub resources: ResourceRequestProfile,
    /// Values that Code can actually apply after host-level clamping.
    #[serde(default)]
    pub effective_resources: ResourceRequestProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamp_reason: Option<String>,
    #[serde(default)]
    pub scope: ResourceGrantScope,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestResourcesEvent {
    pub call_id: String,
    #[serde(default)]
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub current: ResourceRequestProfile,
    pub requested: ResourceRequestProfile,
}
