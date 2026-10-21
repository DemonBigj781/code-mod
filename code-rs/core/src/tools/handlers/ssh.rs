use crate::codex::Session;
use crate::codex::ToolCallCtx;
use crate::exec::ExecParams;
use crate::ssh_registry::{AuthMethod, Connection, Credentials, Registry};
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::tool_error;
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use async_trait::async_trait;
use code_protocol::models::FunctionCallOutputBody;
use code_protocol::models::PermissionProfile;
use code_protocol::models::ResponseInputItem;
use code_protocol::models::SandboxPermissions;
use code_protocol::models::ShellToolCallParams;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
const MAX_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
const MIN_TIMEOUT_MS: u64 = 1_000;
const SSH_CONNECT_TIMEOUT_SECONDS: &str = "10";
const ASKPASS_ENV: &str = "CODE_SSH_ASKPASS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshRequest {
    pub(crate) name: String,
    pub(crate) auth: Option<AuthMethod>,
    pub(crate) command: String,
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SshToolArguments {
    name: String,
    #[serde(default)]
    auth: Option<String>,
    command: String,
    #[serde(default, alias = "timeout")]
    timeout_ms: Option<u64>,
}

pub(crate) struct SshToolHandler;

#[async_trait]
impl ToolHandler for SshToolHandler {
    async fn handle(
        &self,
        sess: &Session,
        turn_diff_tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(inv.ctx.call_id, "ssh expects function-call arguments");
        };

        let parsed: SshToolArguments = match serde_json::from_str(&arguments) {
            Ok(value) => value,
            Err(error) => {
                return tool_error(inv.ctx.call_id, format!("invalid ssh arguments: {error}"));
            }
        };

        let auth = match parse_auth(parsed.auth.as_deref()) {
            Ok(auth) => auth,
            Err(error) => return tool_error(inv.ctx.call_id, error),
        };

        dispatch_ssh_request(
            sess,
            turn_diff_tracker,
            &inv.ctx,
            SshRequest {
                name: parsed.name,
                auth,
                command: parsed.command,
                timeout_ms: parsed.timeout_ms,
            },
            inv.attempt_req,
        )
        .await
    }
}

pub(crate) async fn handle_shell_alias(
    sess: &Session,
    turn_diff_tracker: &mut TurnDiffTracker,
    ctx: &ToolCallCtx,
    params: ShellToolCallParams,
    attempt_req: u64,
) -> ResponseInputItem {
    let Some(request) = parse_shell_alias(&params.command, params.timeout_ms) else {
        return tool_error(
            ctx.call_id.clone(),
            "shell ssh requires a registered name and command",
        );
    };

    dispatch_ssh_request(sess, turn_diff_tracker, ctx, request, attempt_req).await
}

pub(crate) fn is_shell_alias(command: &[String]) -> bool {
    command.first().is_some_and(|value| value == "ssh")
}

fn parse_shell_alias(command: &[String], timeout_ms: Option<u64>) -> Option<SshRequest> {
    if !is_shell_alias(command) || command.len() < 3 {
        return None;
    }

    let name = command[1].clone();
    let mut rest = &command[2..];
    let mut auth = None;

    if rest.first().is_some_and(|value| value == "--") {
        rest = &rest[1..];
    } else if let Some(value) = rest.first() {
        if matches!(value.as_str(), "pwd" | "key") {
            auth = parse_auth(Some(value.as_str())).ok()?;
            rest = &rest[1..];
            if rest.first().is_some_and(|value| value == "--") {
                rest = &rest[1..];
            }
        }
    }

    if rest.is_empty() {
        return None;
    }

    Some(SshRequest {
        name,
        auth,
        command: rest.join(" "),
        timeout_ms,
    })
}

fn parse_auth(value: Option<&str>) -> Result<Option<AuthMethod>, String> {
    value
        .map(AuthMethod::parse)
        .transpose()
        .map_err(|error| format!("invalid ssh auth selector: {error}"))
}

async fn dispatch_ssh_request(
    sess: &Session,
    turn_diff_tracker: &mut TurnDiffTracker,
    ctx: &ToolCallCtx,
    request: SshRequest,
    attempt_req: u64,
) -> ResponseInputItem {
    if request.command.trim().is_empty() {
        return tool_error(ctx.call_id.clone(), "ssh requires a non-empty command");
    }

    let timeout_ms = request
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);

    if crate::ssh_registry::is_local_alias(&request.name) {
        let command = sess
            .user_shell()
            .shell_script_invocation_or_default(request.command, false, false);
        let params = ExecParams {
            command,
            shell_script: None,
            cwd: sess.get_cwd().to_path_buf(),
            timeout_ms: Some(timeout_ms),
            env: sess.execution_environment(),
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
        };
        return crate::codex::exec_tool::handle_container_exec_with_params(
            params,
            sess,
            turn_diff_tracker,
            ctx,
            attempt_req,
        )
        .await;
    }

    let registry = Registry::new(sess.code_home());
    let connection = match registry.get(&request.name) {
        Ok(connection) => connection,
        Err(error) => return tool_error(ctx.call_id.clone(), error.to_string()),
    };
    let credentials = match registry.credentials(&connection, request.auth) {
        Ok(credentials) => credentials,
        Err(error) => return tool_error(ctx.call_id.clone(), error.to_string()),
    };
    let askpass = match askpass_path_for(&credentials, sess.code_home()) {
        Ok(path) => path,
        Err(error) => return tool_error(ctx.call_id.clone(), error),
    };

    let mut last_transport_failure = None;
    for endpoint in connection.ordered_endpoints() {
        let params = match remote_exec_params(
            sess,
            &connection,
            endpoint,
            &credentials,
            askpass.as_deref(),
            &request.command,
            timeout_ms,
        ) {
            Ok(params) => params,
            Err(error) => return tool_error(ctx.call_id.clone(), error),
        };
        let response = crate::codex::exec_tool::handle_container_exec_with_params(
            params,
            sess,
            turn_diff_tracker,
            ctx,
            attempt_req,
        )
        .await;
        if response_success(&response) != Some(false) {
            return response;
        }
        if !is_retryable_transport_failure(response_text(&response).unwrap_or_default()) {
            return response;
        }
        last_transport_failure = Some(response);
    }

    last_transport_failure.unwrap_or_else(|| {
        tool_error(
            ctx.call_id.clone(),
            format!("SSH connection `{}` has no usable endpoints", connection.name),
        )
    })
}

fn remote_exec_params(
    sess: &Session,
    _connection: &Connection,
    endpoint: &crate::ssh_registry::Endpoint,
    credentials: &Credentials,
    askpass: Option<&Path>,
    command: &str,
    timeout_ms: u64,
) -> Result<ExecParams, String> {
    let argv = build_remote_argv(endpoint, credentials, askpass, command)?;

    let mut env = sess.execution_environment();
    if let Some(helper) = askpass {
        env.insert("SSH_ASKPASS".to_owned(), helper.to_string_lossy().into_owned());
        env.insert("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned());
        env.insert("DISPLAY".to_owned(), "code-ssh-askpass".to_owned());
        match credentials {
            Credentials::Password(value) => {
                env.insert(ASKPASS_ENV.to_owned(), value.clone());
            }
            Credentials::Key {
                passphrase: Some(value),
                ..
            } => {
                env.insert(ASKPASS_ENV.to_owned(), value.clone());
            }
            Credentials::Key { passphrase: None, .. } => {}
        }
    }

    Ok(ExecParams {
        command: argv,
        shell_script: None,
        cwd: sess.get_cwd().to_path_buf(),
        timeout_ms: Some(timeout_ms),
        env,
        sandbox_permissions: SandboxPermissions::WithAdditionalPermissions,
        additional_permissions: Some(PermissionProfile {
            network: Some(true),
            ..PermissionProfile::default()
        }),
        justification: Some("Execute a command through a registered SSH connection".to_owned()),
    })
}

fn build_remote_argv(
    endpoint: &crate::ssh_registry::Endpoint,
    credentials: &Credentials,
    askpass: Option<&Path>,
    command: &str,
) -> Result<Vec<String>, String> {
    let mut argv = vec![
        "ssh".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        "-o".to_owned(),
        "ConnectionAttempts=1".to_owned(),
        "-o".to_owned(),
        format!("ConnectTimeout={SSH_CONNECT_TIMEOUT_SECONDS}"),
        "-p".to_owned(),
        endpoint.port.to_string(),
    ];

    let needs_askpass = match credentials {
        Credentials::Password(_) => {
            argv.extend([
                "-o".to_owned(),
                "PreferredAuthentications=password,keyboard-interactive".to_owned(),
                "-o".to_owned(),
                "NumberOfPasswordPrompts=1".to_owned(),
            ]);
            true
        }
        Credentials::Key { path, passphrase } => {
            argv.extend([
                "-o".to_owned(),
                "PreferredAuthentications=publickey".to_owned(),
            ]);
            if let Some(path) = path {
                argv.extend(["-o".to_owned(), "IdentitiesOnly=yes".to_owned()]);
                argv.extend(["-i".to_owned(), path.to_string_lossy().into_owned()]);
            }
            passphrase.is_some()
        }
    };

    if !needs_askpass {
        argv.extend(["-o".to_owned(), "BatchMode=yes".to_owned()]);
    } else if askpass.is_none() {
        return Err("SSH credential requires an askpass helper".to_owned());
    }

    argv.push(endpoint.display());
    argv.push(command.to_owned());
    Ok(argv)
}

fn askpass_path_for(credentials: &Credentials, code_home: &Path) -> Result<Option<PathBuf>, String> {
    let needs_askpass = match credentials {
        Credentials::Password(_) => true,
        Credentials::Key { passphrase, .. } => passphrase.is_some(),
    };
    if !needs_askpass {
        return Ok(None);
    }

    let (filename, contents) = if cfg!(windows) {
        (
            "ssh-askpass.cmd",
            "@echo off\r\necho %CODE_SSH_ASKPASS%\r\n",
        )
    } else {
        (
            "ssh-askpass",
            "#!/bin/sh\nprintf '%s\\n' \"$CODE_SSH_ASKPASS\"\n",
        )
    };
    std::fs::create_dir_all(code_home)
        .map_err(|error| format!("cannot prepare SSH askpass helper: {error}"))?;
    let path = code_home.join(filename);
    if path.is_symlink() {
        return Err("SSH askpass helper must not be a symlink".to_owned());
    }
    if path.exists() {
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("cannot inspect SSH askpass helper: {error}"))?;
        if !metadata.is_file() {
            return Err("SSH askpass helper must be a regular file".to_owned());
        }
        let existing = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read SSH askpass helper: {error}"))?;
        if existing != contents {
            return Err("SSH askpass helper has unexpected contents".to_owned());
        }
    } else {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| format!("cannot create SSH askpass helper: {error}"))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| format!("cannot write SSH askpass helper: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("cannot flush SSH askpass helper: {error}"))?;
    }
    Ok(Some(path))
}

fn response_success(response: &ResponseInputItem) -> Option<bool> {
    match response {
        ResponseInputItem::FunctionCallOutput { output, .. } => output.success,
        _ => None,
    }
}

fn response_text(response: &ResponseInputItem) -> Option<&str> {
    match response {
        ResponseInputItem::FunctionCallOutput { output, .. } => {
            match &output.body {
                FunctionCallOutputBody::Text(text) => Some(text),
                FunctionCallOutputBody::ContentItems(_) => None,
            }
        }
        _ => None,
    }
}

fn is_retryable_transport_failure(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "connection refused",
        "connection timed out",
        "operation timed out",
        "no route to host",
        "network is unreachable",
        "could not resolve hostname",
        "name or service not known",
        "temporary failure in name resolution",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_registry::Endpoint;

    #[test]
    fn shell_alias_preserves_pwd_as_a_command_after_separator() {
        let request = parse_shell_alias(
            &["ssh".into(), "mint".into(), "--".into(), "pwd".into()],
            None,
        )
        .expect("alias should parse");
        assert_eq!(request.auth, None);
        assert_eq!(request.command, "pwd");
    }

    #[test]
    fn shell_alias_rejects_auth_selector_without_a_command() {
        assert!(parse_shell_alias(&["ssh".into(), "mint".into(), "pwd".into()], None).is_none());
    }

    #[test]
    fn shell_alias_selects_configured_auth_before_command() {
        let request = parse_shell_alias(
            &["ssh".into(), "mint".into(), "key".into(), "uname -a".into()],
            Some(2000),
        )
        .expect("alias should parse");
        assert_eq!(request.auth, Some(AuthMethod::Key));
        assert_eq!(request.command, "uname -a");
        assert_eq!(request.timeout_ms, Some(2000));
    }

    #[test]
    fn transport_failover_only_matches_connection_failures() {
        assert!(is_retryable_transport_failure(
            "ssh: connect to host example port 22: Connection timed out"
        ));
        assert!(!is_retryable_transport_failure("remote command exited with code 1"));
    }

    #[test]
    fn endpoint_arguments_keep_remote_command_as_one_argument() {
        let endpoint = Endpoint::parse("user@example.test:2222").expect("endpoint");
        let args = build_remote_argv(
            &endpoint,
            &Credentials::Key {
                path: None,
                passphrase: None,
            },
            None,
            "printf '%s' \"a b\"",
        )
        .expect("SSH arguments");
        assert_eq!(args.last().map(String::as_str), Some("printf '%s' \"a b\""));
        assert!(args.windows(2).any(|pair| pair == ["-o", "BatchMode=yes"]));
    }

    #[test]
    fn password_arguments_require_askpass_without_exposing_the_secret() {
        let endpoint = Endpoint::parse("user@example.test").expect("endpoint");
        let args = build_remote_argv(
            &endpoint,
            &Credentials::Password("not-for-argv".to_owned()),
            Some(Path::new("/tmp/ssh-askpass")),
            "id",
        )
        .expect("SSH arguments");
        assert!(!args.iter().any(|value| value == "not-for-argv"));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-o",
                "PreferredAuthentications=password,keyboard-interactive"
            ]
        }));
    }
}
