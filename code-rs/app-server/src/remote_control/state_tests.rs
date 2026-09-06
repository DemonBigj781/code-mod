use super::state::RemoteControlEnrollmentRecord;
use super::state::RemoteControlState;
use sqlx::Row;

fn enrollment(
    account_id: &str,
    client_name: Option<&str>,
    server_id: &str,
    enabled: Option<bool>,
) -> RemoteControlEnrollmentRecord {
    RemoteControlEnrollmentRecord {
        websocket_url: "wss://example.com/backend-api/wham/remote/control/server".to_string(),
        account_id: account_id.to_string(),
        app_server_client_name: client_name.map(str::to_string),
        server_id: server_id.to_string(),
        environment_id: format!("env-{server_id}"),
        server_name: format!("server-{server_id}"),
        remote_control_enabled: enabled,
    }
}

#[tokio::test]
async fn open_is_idempotent_and_creates_a_secret_free_wal_database() {
    let code_home = tempfile::tempdir().expect("create temp code home");

    let first = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    drop(first);
    let reopened = RemoteControlState::open(code_home.path())
        .await
        .expect("reopen state");
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&reopened.pool)
        .await
        .expect("read journal mode");
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&reopened.pool)
        .await
        .expect("read synchronous mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 1);

    let columns = sqlx::query("PRAGMA table_info(remote_control_enrollments)")
        .fetch_all(&reopened.pool)
        .await
        .expect("read enrollment columns")
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert_eq!(
        columns,
        vec![
            "websocket_url",
            "account_id",
            "app_server_client_name",
            "server_id",
            "environment_id",
            "server_name",
            "remote_control_enabled",
            "updated_at",
        ]
    );
    assert!(columns.iter().all(|column| {
        !column.contains("token") && !column.contains("secret") && !column.contains("expir")
    }));
}

#[tokio::test]
async fn enrollment_round_trips_and_isolated_by_account_and_client_name() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let account_a = enrollment("account-a", Some("desktop"), "a", Some(true));
    let account_b = enrollment("account-b", Some("desktop"), "b", Some(false));
    let unnamed = enrollment("account-a", None, "unnamed", None);

    state
        .upsert_enrollment(&account_a)
        .await
        .expect("insert account a");
    state
        .upsert_enrollment(&account_b)
        .await
        .expect("insert account b");
    state
        .upsert_enrollment(&unnamed)
        .await
        .expect("insert unnamed client");

    assert_eq!(
        state
            .get_enrollment(&account_a.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load account a"),
        Some(account_a.clone())
    );
    assert_eq!(
        state
            .get_enrollment(&account_b.websocket_url, "account-b", Some("desktop"))
            .await
            .expect("load account b"),
        Some(account_b)
    );
    assert_eq!(
        state
            .get_enrollment(&unnamed.websocket_url, "account-a", None)
            .await
            .expect("load unnamed client"),
        Some(unnamed)
    );
    assert_eq!(
        state
            .get_enrollment(&account_a.websocket_url, "account-a", Some("other"))
            .await
            .expect("load other client"),
        None
    );
}

#[tokio::test]
async fn reenrollment_updates_identity_without_overwriting_enabled_preference() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let original = enrollment("account-a", Some("desktop"), "old", Some(true));
    let replacement = enrollment("account-a", Some("desktop"), "new", Some(false));

    state
        .upsert_enrollment(&original)
        .await
        .expect("insert enrollment");
    state
        .upsert_enrollment(&replacement)
        .await
        .expect("replace enrollment identity");

    let stored = state
        .get_enrollment(&original.websocket_url, "account-a", Some("desktop"))
        .await
        .expect("load enrollment")
        .expect("enrollment exists");
    assert_eq!(stored.server_id, "new");
    assert_eq!(stored.environment_id, "env-new");
    assert_eq!(stored.server_name, "server-new");
    assert_eq!(stored.remote_control_enabled, Some(true));
}

#[tokio::test]
async fn set_enabled_and_delete_only_touch_the_matching_composite_key() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let desktop = enrollment("account-a", Some("desktop"), "desktop", Some(false));
    let mobile = enrollment("account-a", Some("mobile"), "mobile", Some(false));

    state
        .upsert_enrollment(&desktop)
        .await
        .expect("insert desktop");
    state
        .upsert_enrollment(&mobile)
        .await
        .expect("insert mobile");

    assert_eq!(
        state
            .set_enabled(&desktop.websocket_url, "account-a", Some("desktop"), true)
            .await
            .expect("enable desktop"),
        1
    );
    assert_eq!(
        state
            .get_enrollment(&desktop.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load desktop")
            .expect("desktop exists")
            .remote_control_enabled,
        Some(true)
    );
    assert_eq!(
        state
            .get_enrollment(&mobile.websocket_url, "account-a", Some("mobile"))
            .await
            .expect("load mobile")
            .expect("mobile exists")
            .remote_control_enabled,
        Some(false)
    );
    assert_eq!(
        state
            .delete_enrollment(&desktop.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("delete desktop"),
        1
    );
    assert_eq!(
        state
            .get_enrollment(&desktop.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load deleted desktop"),
        None
    );
    assert!(
        state
            .get_enrollment(&mobile.websocket_url, "account-a", Some("mobile"))
            .await
            .expect("load retained mobile")
            .is_some()
    );
}
