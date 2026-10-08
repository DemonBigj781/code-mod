use super::*;

pub(super) struct SpawnAgentProcessRequest<'a> {
    pub(super) agent_id: &'a str,
    pub(super) model: &'a str,
    pub(super) read_only: bool,
    pub(super) working_dir: Option<PathBuf>,
    pub(super) use_current_exe: bool,
    pub(super) command: &'a str,
    pub(super) command_for_spawn: &'a str,
    pub(super) final_args: &'a [String],
    pub(super) env: &'a std::collections::HashMap<String, String>,
}

pub(super) async fn run_agent_process(
    request: SpawnAgentProcessRequest<'_>,
) -> Result<(std::process::ExitStatus, String, String), String> {
    if let Some(message) = crate::process_execution::unsupported_process_tool_message() {
        return Err(message.to_owned());
    }

    let SpawnAgentProcessRequest {
        agent_id,
        model,
        read_only,
        working_dir,
        use_current_exe,
        command,
        command_for_spawn,
        final_args,
        env,
    } = request;

    use crate::protocol::SandboxPolicy;
    use crate::spawn::StdioPolicy;

    if read_only {
        // Read-only path: must honor resolve_program_path (and CODE_BINARY_PATH) just
        // like the write path; skipping this can regress to PATH resolution and
        // launch the npm shim on Windows (issue #497).
        let program = resolve_program_path(use_current_exe, command_for_spawn)?;
        let mut cmd = Command::new(program);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        cmd.args(final_args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }

        // Ensure the child is terminated if this process dies unexpectedly.
        cmd.kill_on_drop(true);

        match spawn_tokio_command_with_retry(&mut cmd).await {
            Ok(child) => process_output::stream_child_output(agent_id, child).await,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    return Err(runtime_paths::format_agent_not_found_error(
                        command,
                        command_for_spawn,
                    ));
                }

                Err(format!("Failed to execute {model}: {err}"))
            }
        }
    } else {
        // Resolve the command and args we prepared above into Vec<String> for spawn helpers.
        let program = resolve_program_path(use_current_exe, command_for_spawn)?;
        let args = final_args.to_vec();

        let child_result: std::io::Result<tokio::process::Child> = crate::spawn::spawn_child_async(
            program.clone(),
            args,
            Some(program.to_string_lossy().as_ref()),
            working_dir.clone().unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            }),
            &SandboxPolicy::DangerFullAccess,
            StdioPolicy::RedirectForShellTool,
            env.clone(),
        )
        .await;

        match child_result {
            Ok(child) => process_output::stream_child_output(agent_id, child).await,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    return Err(runtime_paths::format_agent_not_found_error(
                        command,
                        command_for_spawn,
                    ));
                }
                Err(format!("Failed to spawn sandboxed agent: {err}"))
            }
        }
    }
}

#[cfg(all(test, feature = "cosmopolitan-ape", target_os = "linux"))]
mod cosmopolitan_tests {
    use super::{SpawnAgentProcessRequest, run_agent_process};
    use std::collections::HashMap;

    #[tokio::test]
    async fn cosmopolitan_build_does_not_start_an_unsandboxed_agent_process() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker = directory.path().join("agent-started");
        let args = vec![
            "-c".to_owned(),
            "printf started > \"$APE_MARKER\"".to_owned(),
        ];
        let env = HashMap::from([(
            "APE_MARKER".to_owned(),
            marker.to_string_lossy().into_owned(),
        )]);

        let result = run_agent_process(SpawnAgentProcessRequest {
            agent_id: "cosmopolitan-process-guard",
            model: "fixture",
            read_only: true,
            working_dir: Some(directory.path().to_path_buf()),
            use_current_exe: false,
            command: "/bin/sh",
            command_for_spawn: "/bin/sh",
            final_args: &args,
            env: &env,
        })
        .await;

        assert!(result.is_err(), "APE unexpectedly launched the external process");
        assert!(!marker.exists(), "external command ran without a validated sandbox");
    }
}
