use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use clap::Parser;
use clap::Subcommand;
use code_app_server_daemon::BackendKind;
use code_app_server_daemon::LifecycleOutput;
use code_app_server_daemon::LifecycleStatus;
use code_app_server_daemon::RemoteControlReadyOutput;
use code_app_server_daemon::RemoteControlReadyStatus;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[derive(Debug, Parser)]
#[command(name = "code")]
struct TestCli {
    #[command(subcommand)]
    subcommand: TestSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestSubcommand {
    RemoteControl(RemoteControlCommand),
}

fn parse_remote_control(args: &[&str]) -> RemoteControlCommand {
    let cli = TestCli::try_parse_from(args).expect("remote-control arguments should parse");
    let TestSubcommand::RemoteControl(command) = cli.subcommand;
    command
}

fn ready_status(status: RemoteControlConnectionStatus) -> RemoteControlReadyStatus {
    RemoteControlReadyStatus {
        status,
        server_name: "steam-deck".to_string(),
        installation_id: "11111111-1111-4111-8111-111111111111".to_string(),
        environment_id: Some("env_test".to_string()),
    }
}

fn lifecycle_output(status: LifecycleStatus) -> LifecycleOutput {
    LifecycleOutput {
        status,
        backend: Some(BackendKind::Pid),
        pid: Some(42),
        socket_path: PathBuf::from("/tmp/code-app-server.sock"),
        cli_version: "1.0.0".to_string(),
        app_server_version: Some("2.0.0".to_string()),
    }
}

fn daemon_ready_output(status: RemoteControlConnectionStatus) -> RemoteControlReadyOutput {
    RemoteControlReadyOutput {
        daemon: lifecycle_output(LifecycleStatus::Started),
        remote_control: ready_status(status),
    }
}

fn pairing_response(manual_pairing_code: Option<&str>) -> RemoteControlPairingStartResponse {
    RemoteControlPairingStartResponse {
        pairing_code: "pairing-code".to_string(),
        manual_pairing_code: manual_pairing_code.map(str::to_string),
        environment_id: "env_test".to_string(),
        expires_at: 1_700_000_000,
    }
}

#[test]
fn remote_control_foreground_parses_with_optional_json() {
    let plain = parse_remote_control(&["code", "remote-control"]);
    assert_eq!(plain.subcommand_name(), "remote-control");
    assert!(!plain.json());

    let json = parse_remote_control(&["code", "remote-control", "--json"]);
    assert_eq!(json.subcommand_name(), "remote-control");
    assert!(json.json());
}

#[test]
fn remote_control_subcommands_parse_with_global_json_in_both_positions() {
    for (name, expected) in [
        ("start", "remote-control start"),
        ("stop", "remote-control stop"),
        ("pair", "remote-control pair"),
    ] {
        let before = parse_remote_control(&["code", "remote-control", "--json", name]);
        assert_eq!(before.subcommand_name(), expected);
        assert!(before.json());

        let after = parse_remote_control(&["code", "remote-control", name, "--json"]);
        assert_eq!(after.subcommand_name(), expected);
        assert!(after.json());
    }
}

#[test]
fn remote_control_rejects_unexpected_positional_arguments() {
    assert!(
        TestCli::try_parse_from(["code", "remote-control", "unexpected"]).is_err(),
        "unexpected positional argument should fail"
    );
    assert!(
        TestCli::try_parse_from(["code", "remote-control", "start", "unexpected"]).is_err(),
        "subcommand positional argument should fail"
    );
}

#[test]
fn remote_control_rejects_remote_provider_flags() {
    assert!(
        TestCli::try_parse_from([
            "code",
            "--remote",
            "unix:///tmp/remote.sock",
            "remote-control",
        ])
        .is_err(),
        "root remote-provider mode should fail"
    );
    assert!(
        TestCli::try_parse_from([
            "code",
            "remote-control",
            "--remote",
            "unix:///tmp/remote.sock",
        ])
        .is_err(),
        "remote-control remote-provider mode should fail"
    );
}

#[test]
fn remote_control_foreground_human_output_is_stable() {
    assert_eq!(
        format_foreground_ready_output(
            &ready_status(RemoteControlConnectionStatus::Connected),
            false,
        )
        .expect("foreground output"),
        "This machine is available for remote control as steam-deck.\nPress Ctrl-C to stop."
    );
}

#[test]
fn remote_control_foreground_json_output_has_stable_fields() {
    let output = format_foreground_ready_output(
        &ready_status(RemoteControlConnectionStatus::Connected),
        true,
    )
    .expect("foreground JSON output");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).expect("valid JSON"),
        json!({
            "mode": "foreground",
            "status": "connected",
            "serverName": "steam-deck",
            "environmentId": "env_test",
            "timedOut": false,
        })
    );
}

#[test]
fn remote_control_connecting_json_is_marked_timed_out() {
    let foreground = format_foreground_ready_output(
        &ready_status(RemoteControlConnectionStatus::Connecting),
        true,
    )
    .expect("foreground connecting JSON output");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&foreground).expect("valid JSON"),
        json!({
            "mode": "foreground",
            "status": "connecting",
            "serverName": "steam-deck",
            "environmentId": "env_test",
            "timedOut": true,
        })
    );

    let daemon = format_remote_control_start_output(
        &daemon_ready_output(RemoteControlConnectionStatus::Connecting),
        true,
    )
    .expect("daemon connecting JSON output");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&daemon)
            .expect("valid daemon connecting JSON"),
        json!({
            "mode": "daemon",
            "status": "connecting",
            "serverName": "steam-deck",
            "environmentId": "env_test",
            "timedOut": true,
            "daemon": {
                "status": "started",
                "backend": "pid",
                "pid": 42,
                "socketPath": "/tmp/code-app-server.sock",
                "cliVersion": "1.0.0",
                "appServerVersion": "2.0.0",
            },
        })
    );
}

#[test]
fn remote_control_daemon_json_includes_lifecycle_output() {
    let output = format_remote_control_start_output(
        &daemon_ready_output(RemoteControlConnectionStatus::Connected),
        true,
    )
    .expect("daemon JSON output");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).expect("valid JSON"),
        json!({
            "mode": "daemon",
            "status": "connected",
            "serverName": "steam-deck",
            "environmentId": "env_test",
            "timedOut": false,
            "daemon": {
                "status": "started",
                "backend": "pid",
                "pid": 42,
                "socketPath": "/tmp/code-app-server.sock",
                "cliVersion": "1.0.0",
                "appServerVersion": "2.0.0",
            },
        })
    );
}

#[test]
fn remote_control_daemon_human_output_does_not_leak_credentials() {
    let output = format_remote_control_start_output(
        &daemon_ready_output(RemoteControlConnectionStatus::Connected),
        false,
    )
    .expect("daemon human output");
    assert!(output.contains("This machine is available for remote control as steam-deck."));
    assert!(output.contains("Daemon status: started."));
    assert!(output.contains("App-server PID: 42."));
    assert!(!output.contains("Bearer"));
    assert!(!output.contains("remote_control_token"));
    assert!(!output.contains("server-scoped"));
}

#[test]
fn remote_control_start_rejects_terminal_failure_statuses() {
    assert_eq!(
        format_foreground_ready_output(
            &ready_status(RemoteControlConnectionStatus::Errored),
            false,
        )
        .expect_err("errored status should fail")
        .to_string(),
        "Remote control could not connect on steam-deck. Verify that you are signed in with ChatGPT using `code login`; API key authentication is not supported."
    );
    assert_eq!(
        format_foreground_ready_output(
            &ready_status(RemoteControlConnectionStatus::Disabled),
            false,
        )
        .expect_err("disabled status should fail")
        .to_string(),
        "Remote control is disabled on steam-deck."
    );
}

#[tokio::test]
async fn remote_control_pairing_waits_for_readiness() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let ready_calls = Arc::clone(&calls);
    let pairing_calls = Arc::clone(&calls);

    let pairing = start_remote_control_pairing_after_ready(
        move || async move {
            ready_calls.lock().expect("ready call log").push("ready");
            Ok(daemon_ready_output(RemoteControlConnectionStatus::Connected))
        },
        move || async move {
            let mut calls = pairing_calls.lock().expect("pairing call log");
            assert_eq!(calls.as_slice(), ["ready"]);
            calls.push("pair");
            Ok(pairing_response(Some("ABCD-EFGH")))
        },
    )
    .await
    .expect("pairing should start after readiness");

    assert_eq!(pairing.manual_pairing_code.as_deref(), Some("ABCD-EFGH"));
    assert_eq!(calls.lock().expect("final call log").as_slice(), ["ready", "pair"]);
}

#[test]
fn remote_control_stop_outputs_human_and_json_status() {
    let output = lifecycle_output(LifecycleStatus::NotRunning);
    assert_eq!(
        format_remote_control_stop_output(&output, false).expect("human stop output"),
        "Remote control is not running."
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &format_remote_control_stop_output(&output, true).expect("JSON stop output")
        )
        .expect("valid JSON"),
        json!({
            "status": "notRunning",
            "backend": "pid",
            "pid": 42,
            "socketPath": "/tmp/code-app-server.sock",
            "cliVersion": "1.0.0",
            "appServerVersion": "2.0.0",
        })
    );
}

#[test]
fn remote_control_pairing_outputs_manual_code_and_stable_json() {
    let output = pairing_response(Some("ABCD-EFGH"));
    assert_eq!(
        format_remote_control_pairing_output(&output, false).expect("human pairing output"),
        "Pairing code: ABCD-EFGH"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &format_remote_control_pairing_output(&output, true).expect("JSON pairing output")
        )
        .expect("valid JSON"),
        json!({
            "pairingCode": "pairing-code",
            "manualPairingCode": "ABCD-EFGH",
            "environmentId": "env_test",
            "expiresAt": 1_700_000_000,
        })
    );
}

#[test]
fn remote_control_pairing_human_output_requires_manual_code() {
    assert_eq!(
        format_remote_control_pairing_output(&pairing_response(None), false)
            .expect_err("missing manual pairing code should fail")
            .to_string(),
        "remote-control pairing response did not include a manual pairing code"
    );
}

#[tokio::test]
async fn remote_control_foreground_start_stops_before_readiness() {
    let mut app_server_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    stop_tx.send(true).expect("send stop signal");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_foreground_remote_control_start(
            &mut app_server_task,
            std::future::pending::<anyhow::Result<RemoteControlReadyStatus>>(),
            stop_rx,
        ),
    )
    .await
    .expect("startup wait should return after stop signal");

    assert!(matches!(result, ForegroundStartupResult::Stopped));
    app_server_task.abort();
    let _ = app_server_task.await;
}

#[tokio::test]
async fn remote_control_foreground_start_reports_early_app_server_exit() {
    let mut app_server_task =
        tokio::spawn(async { Err(std::io::Error::other("failed before socket bind")) });
    let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_foreground_remote_control_start(
            &mut app_server_task,
            std::future::pending::<anyhow::Result<RemoteControlReadyStatus>>(),
            stop_rx,
        ),
    )
    .await
    .expect("startup wait should return after app-server exit");

    let ForegroundStartupResult::AppServerExited(error) = result else {
        panic!("expected app-server exit before readiness");
    };
    assert_eq!(
        error.to_string(),
        "foreground app-server exited before remote control became ready"
    );
}

#[tokio::test]
async fn remote_control_foreground_start_returns_readiness() {
    let expected = ready_status(RemoteControlConnectionStatus::Connected);
    let mut app_server_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
    let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    let result = wait_for_foreground_remote_control_start(
        &mut app_server_task,
        std::future::ready(Ok(expected.clone())),
        stop_rx,
    )
    .await;

    let ForegroundStartupResult::Ready(actual) = result else {
        panic!("expected readiness result");
    };
    assert_eq!(actual, expected);
    app_server_task.abort();
    let _ = app_server_task.await;
}

#[tokio::test]
async fn remote_control_foreground_wait_aborts_on_stop_signal() {
    let app_server_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    stop_tx.send(true).expect("send stop signal");

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_foreground_app_server(app_server_task, stop_rx),
    )
    .await
    .expect("foreground wait should return after stop signal")
    .expect("stop signal should shut down cleanly");
}
