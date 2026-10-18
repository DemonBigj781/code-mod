use crate::config::ReplRuntimeKindToml;
use crate::model_family::ModelFamily;
use crate::protocol::AskForApproval;
use crate::protocol::SandboxPolicy;
use crate::tool_apply_patch::ApplyPatchToolType;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;

#[derive(Debug, Clone)]
pub enum ConfigShellToolType {
    DefaultShell,
    ShellWithRequest { sandbox_policy: SandboxPolicy },
    LocalShell,
    StreamableShell,
}

#[derive(Debug, Clone)]
pub struct ToolsConfig {
    pub shell_type: ConfigShellToolType,
    pub plan_tool: bool,
    pub apply_patch_tool_type: Option<ApplyPatchToolType>,
    pub web_search_request: bool,
    pub web_search_external: bool,
    pub search_tool: bool,
    pub repl: bool,
    /// Runtimes that passed the health probe and should be exposed as
    /// individual `repl_{label}` tools. Empty when `repl` is false.
    pub repl_available_runtimes: Vec<ReplRuntimeKindToml>,
    pub include_view_image_tool: bool,
    pub web_search_allowed_domains: Option<Vec<String>>,
    pub agent_model_allowed_values: Vec<String>,
}

pub struct ToolsConfigParams<'a> {
    pub model_family: &'a ModelFamily,
    pub approval_policy: AskForApproval,
    pub sandbox_policy: SandboxPolicy,
    pub include_plan_tool: bool,
    pub include_apply_patch_tool: bool,
    pub include_web_search_request: bool,
    pub use_streamable_shell_tool: bool,
    pub include_view_image_tool: bool,
}

impl ToolsConfig {
    pub fn new(params: ToolsConfigParams<'_>) -> Self {
        let ToolsConfigParams {
            model_family,
            approval_policy,
            sandbox_policy,
            include_plan_tool,
            include_apply_patch_tool,
            include_web_search_request,
            use_streamable_shell_tool,
            include_view_image_tool,
        } = params;
        let shell_type =
            if matches!(approval_policy, AskForApproval::OnRequest) && !use_streamable_shell_tool {
                ConfigShellToolType::ShellWithRequest { sandbox_policy }
            } else if use_streamable_shell_tool {
                ConfigShellToolType::StreamableShell
            } else if model_family.uses_local_shell_tool {
                ConfigShellToolType::LocalShell
            } else {
                ConfigShellToolType::DefaultShell
            };

        let apply_patch_tool_type = if include_apply_patch_tool {
            // On Windows, grammar-based apply_patch invocations rely on heredocs
            // the shell cannot parse. Force the JSON/function variant instead.
            #[cfg(target_os = "windows")]
            {
                model_family
                    .apply_patch_tool_type
                    .clone()
                    .map(|_| ApplyPatchToolType::Function)
            }
            #[cfg(not(target_os = "windows"))]
            {
                model_family.apply_patch_tool_type.clone()
            }
        } else {
            None
        };

        Self {
            shell_type,
            plan_tool: include_plan_tool,
            apply_patch_tool_type,
            web_search_request: include_web_search_request,
            web_search_external: true,
            search_tool: false,
            repl: false,
            repl_available_runtimes: Vec::new(),
            include_view_image_tool,
            web_search_allowed_domains: None,
            agent_model_allowed_values: Vec::new(),
        }
    }

    pub fn set_agent_models(&mut self, mut models: Vec<String>, session_id: uuid::Uuid) {
        models = models
            .into_iter()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        models.sort_by(|a, b| {
            a.to_ascii_lowercase()
                .cmp(&b.to_ascii_lowercase())
                .then_with(|| a.cmp(b))
        });
        models.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        // Both session dispatch and provider tool schemas use the same session seed.
        // Rebuilding a prompt must not invalidate the choices just advertised to it.
        let mut seed = [0_u8; 32];
        seed[..16].copy_from_slice(session_id.as_bytes());
        seed[16..].copy_from_slice(session_id.as_bytes());
        self.agent_model_allowed_values = if models.len() > 5 {
            models
                .choose_multiple(&mut StdRng::from_seed(seed), 5)
                .cloned()
                .collect()
        } else {
            models
        };
    }

    pub fn agent_models(&self) -> &[String] {
        &self.agent_model_allowed_values
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ToolsConfig {
        let family = crate::model_family::derive_default_model_family("gpt-5.1-codex");
        ToolsConfig::new(ToolsConfigParams {
            model_family: &family,
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::ReadOnly,
            include_plan_tool: false,
            include_apply_patch_tool: false,
            include_web_search_request: false,
            use_streamable_shell_tool: false,
            include_view_image_tool: false,
        })
    }

    #[test]
    fn agent_shortlist_is_bounded_stable_and_derived_only_from_allowed_models() {
        let names = (0..46).map(|n| format!("model-{n}")).collect::<Vec<_>>();
        let id = uuid::Uuid::from_u128(42);
        let mut session = config();
        let mut prompt = config();
        session.set_agent_models(names.clone(), id);
        let mut reversed = names.clone();
        reversed.reverse();
        prompt.set_agent_models(reversed, id);
        assert_eq!(session.agent_models(), prompt.agent_models());
        assert_eq!(session.agent_models().len(), 5);
        let unique = session
            .agent_models()
            .iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 5);
        assert!(session.agent_models().iter().all(|m| names.contains(m)));
        let original = session.agent_models().to_vec();
        assert!((43..53).any(|n| {
            prompt.set_agent_models(names.clone(), uuid::Uuid::from_u128(n));
            prompt.agent_models() != original.as_slice()
        }));
    }

    #[test]
    fn agent_shortlist_respects_disabled_small_and_duplicate_pools() {
        let mut config = config();
        let id = uuid::Uuid::nil();
        config.set_agent_models(vec![], id);
        assert!(config.agent_models().is_empty());
        config.set_agent_models(
            vec!["one".into(), "ONE".into(), "two".into(), " ".into()],
            id,
        );
        assert_eq!(config.agent_models().len(), 2);
    }
}
