use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::config_types::McpServerConfig;
use crate::config_types::McpServerSchedulingToml;
use crate::config_types::McpServerTransportConfig;

pub const RUSTWRIGHT_MCP_SERVER_NAME: &str = "rustwright";

const RUSTWRIGHT_MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const RUSTWRIGHT_MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

pub fn insert_builtin_server(
    servers: &mut HashMap<String, McpServerConfig>,
    workspace: &Path,
    code_executable: &Path,
) -> bool {
    if servers
        .keys()
        .any(|name| name.eq_ignore_ascii_case(RUSTWRIGHT_MCP_SERVER_NAME))
    {
        return false;
    }

    servers.insert(
        RUSTWRIGHT_MCP_SERVER_NAME.to_owned(),
        builtin_server_config(workspace, code_executable),
    );
    true
}

pub fn builtin_server_config(workspace: &Path, code_executable: &Path) -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: code_executable.to_string_lossy().into_owned(),
            args: vec!["rustwright-mcp".to_owned()],
            env: Some(HashMap::from([(
                "RUSTWRIGHT_MCP_WORKSPACE".to_owned(),
                workspace.to_string_lossy().into_owned(),
            )])),
        },
        startup_timeout_sec: Some(RUSTWRIGHT_MCP_STARTUP_TIMEOUT),
        tool_timeout_sec: Some(RUSTWRIGHT_MCP_TOOL_TIMEOUT),
        scheduling: McpServerSchedulingToml::default(),
        tool_scheduling: BTreeMap::new(),
        disabled_tools: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_config_launches_the_current_code_binary() {
        let config =
            builtin_server_config(Path::new("/tmp/workspace"), Path::new("/opt/code/bin/code"));

        let McpServerTransportConfig::Stdio { command, args, env } = config.transport else {
            panic!("expected stdio transport");
        };

        assert_eq!(command, "/opt/code/bin/code");
        assert_eq!(args, ["rustwright-mcp"]);
        assert_eq!(
            env,
            Some(HashMap::from([(
                "RUSTWRIGHT_MCP_WORKSPACE".to_owned(),
                "/tmp/workspace".to_owned(),
            )]))
        );
        assert_eq!(
            config.startup_timeout_sec,
            Some(RUSTWRIGHT_MCP_STARTUP_TIMEOUT)
        );
        assert_eq!(config.tool_timeout_sec, Some(RUSTWRIGHT_MCP_TOOL_TIMEOUT));
        assert_eq!(config.scheduling, McpServerSchedulingToml::default());
    }

    #[test]
    fn user_defined_rustwright_server_wins_case_insensitively() {
        let mut servers = HashMap::from([(
            "RustWright".to_owned(),
            builtin_server_config(Path::new("/user"), Path::new("/user/code")),
        )]);

        let inserted = insert_builtin_server(
            &mut servers,
            Path::new("/builtin"),
            Path::new("/builtin/code"),
        );

        assert!(!inserted);
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("RustWright"));
    }

    #[test]
    fn inserts_builtin_server_when_no_override_exists() {
        let mut servers = HashMap::new();

        let inserted = insert_builtin_server(
            &mut servers,
            Path::new("/tmp/workspace"),
            Path::new("/opt/code/bin/code"),
        );

        assert!(inserted);
        assert!(servers.contains_key(RUSTWRIGHT_MCP_SERVER_NAME));
    }
}
