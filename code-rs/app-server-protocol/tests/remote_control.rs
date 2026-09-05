use code_app_server_protocol::ClientRequest;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlStatusChangedNotification;
use code_app_server_protocol::ServerNotification;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn remote_control_request_wire_names_deserialize() {
    let cases = [
        ("remoteControl/enable", json!({"ephemeral": true})),
        ("remoteControl/disable", json!({"ephemeral": true})),
        ("remoteControl/status/read", serde_json::Value::Null),
        (
            "remoteControl/pairing/start",
            json!({"manualCode": true}),
        ),
        (
            "remoteControl/pairing/status",
            json!({"pairingCode": "pair_123", "manualPairingCode": null}),
        ),
        (
            "remoteControl/client/list",
            json!({
                "environmentId": "env_123",
                "cursor": null,
                "limit": 25,
                "order": "desc"
            }),
        ),
        (
            "remoteControl/client/revoke",
            json!({"environmentId": "env_123", "clientId": "client_123"}),
        ),
    ];

    for (index, (method, params)) in cases.into_iter().enumerate() {
        let request: ClientRequest = serde_json::from_value(json!({
            "id": index,
            "method": method,
            "params": params,
        }))
        .unwrap_or_else(|err| panic!("failed to deserialize {method}: {err}"));

        assert_eq!(request.method(), method);
    }
}

#[test]
fn remote_control_status_values_use_official_wire_names() {
    let cases = [
        (RemoteControlConnectionStatus::Disabled, "disabled"),
        (RemoteControlConnectionStatus::Connecting, "connecting"),
        (RemoteControlConnectionStatus::Connected, "connected"),
        (RemoteControlConnectionStatus::Errored, "errored"),
    ];

    for (status, expected) in cases {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
    }
}

#[test]
fn remote_control_status_notification_uses_official_wire_name() {
    let notification = ServerNotification::RemoteControlStatusChanged(
        RemoteControlStatusChangedNotification {
            status: RemoteControlConnectionStatus::Connected,
            server_name: "deck".to_string(),
            installation_id: "install_123".to_string(),
            environment_id: Some("env_123".to_string()),
        },
    );

    assert_eq!(
        serde_json::to_value(notification).unwrap(),
        json!({
            "method": "remoteControl/status/changed",
            "params": {
                "status": "connected",
                "serverName": "deck",
                "installationId": "install_123",
                "environmentId": "env_123"
            }
        })
    );
}
