use crate::{Result, Store, files, normalize};
use futures::TryStreamExt;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{Connection, Row, SqliteConnection};
use std::path::Path;
use std::time::Duration;

async fn connect(root: &Path, path: &Path) -> Result<SqliteConnection> {
    let path = files::allowed_path(root, path)?;
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(2));
    let mut connection = SqliteConnection::connect_with(&options).await?;
    sqlx::query("PRAGMA query_only=ON")
        .execute(&mut connection)
        .await?;
    Ok(connection)
}

async fn has_table(connection: &mut SqliteConnection, name: &str) -> Result<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?")
            .bind(name)
            .fetch_optional(connection)
            .await?
            .is_some(),
    )
}

fn text(row: &SqliteRow, field: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(field)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
}
fn number(row: &SqliteRow, field: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(field)
        .ok()
        .flatten()
        .or_else(|| {
            row.try_get::<Option<f64>, _>(field)
                .ok()
                .flatten()
                .map(|v| v as i64)
        })
}
fn millis(row: &SqliteRow, field: &str) -> Option<i64> {
    number(row, field).and_then(|v| normalize::timestamp_ms(&json!(v)))
}

pub(crate) async fn sessions(store: &Store) -> Result<Vec<Value>> {
    let path = store.root.join("state_5.sqlite");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut connection = connect(&store.root, &path).await?;
    if !has_table(&mut connection, "threads").await? {
        return Ok(Vec::new());
    }
    let mut rows = sqlx::query("SELECT * FROM threads ORDER BY id").fetch(&mut connection);
    let mut sessions = Vec::new();
    while let Some(row) = rows.try_next().await? {
        let Some(id) = text(&row, "id") else {
            continue;
        };
        sessions.push(normalize::clean(json!({
            "store_id":store.id,"session_id":id,"ref":format!("{}:{id}",store.id),"source_kind":"sqlite",
            "rollout_path":text(&row,"rollout_path"),"created_at_ms":millis(&row,"created_at_ms").or(millis(&row,"created_at")),
            "updated_at_ms":millis(&row,"updated_at_ms").or(millis(&row,"updated_at")),"cwd":text(&row,"cwd"),
            "title":text(&row,"title").map(code_secrets::redact_secrets),"source":text(&row,"source"),"model_provider":text(&row,"model_provider"),
            "model":text(&row,"model"),"git_branch":text(&row,"git_branch"),"archived":number(&row,"archived").map(|n|n!=0),"history_mode":text(&row,"history_mode")
        })));
    }
    Ok(sessions)
}

pub(crate) async fn has_items(store: &Store, session: &str) -> Result<bool> {
    let path = store.root.join("thread_history_1.sqlite");
    if !path.is_file() {
        return Ok(false);
    }
    let mut connection = connect(&store.root, &path).await?;
    if !has_table(&mut connection, "thread_items").await? {
        return Ok(false);
    }
    Ok(
        sqlx::query("SELECT 1 FROM thread_items WHERE thread_id=? LIMIT 1")
            .bind(session)
            .fetch_optional(&mut connection)
            .await?
            .is_some(),
    )
}

pub(crate) async fn visit_items(
    store: &Store,
    session: &str,
    include_tools: bool,
    visit: &mut dyn FnMut(Value) -> bool,
) -> Result<()> {
    let mut connection = connect(&store.root, &store.root.join("thread_history_1.sqlite")).await?;
    let mut rows = sqlx::query(
        "SELECT * FROM thread_items WHERE thread_id=? ORDER BY rollout_ordinal,item_id",
    )
    .bind(session)
    .fetch(&mut connection);
    while let Some(row) = rows.try_next().await? {
        let Some(raw) = text(&row, "item_json") else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let base = json!({"store_id":store.id,"session_id":session,"turn_id":text(&row,"turn_id"),
            "item_id":text(&row,"item_id").or_else(||payload["id"].as_str().map(ToOwned::to_owned)),
            "ordinal":number(&row,"rollout_ordinal"),"created_at_ms":millis(&row,"created_at_ms"),
            "source_type":text(&row,"item_type").filter(|s|!s.is_empty()).or_else(||payload["type"].as_str().map(ToOwned::to_owned))});
        for item in normalize::projected_items(base, &payload, include_tools) {
            if !visit(item) {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub(crate) async fn turn(store: &Store, session: &str, id: &str) -> Result<Option<Value>> {
    let path = store.root.join("thread_history_1.sqlite");
    if !path.is_file() {
        return Ok(None);
    }
    let mut connection = connect(&store.root, &path).await?;
    if !has_table(&mut connection, "thread_turns").await? {
        return Ok(None);
    }
    let Some(row) = sqlx::query("SELECT * FROM thread_turns WHERE thread_id=? AND turn_id=?")
        .bind(session)
        .bind(id)
        .fetch_optional(&mut connection)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(normalize::clean(
        json!({"store_id":store.id,"session_id":session,"turn_id":id,
        "status":text(&row,"status"),"started_at_ms":millis(&row,"started_at"),"completed_at_ms":millis(&row,"completed_at"),
        "duration_ms":number(&row,"duration_ms"),"ordinal":number(&row,"rollout_ordinal")}),
    )))
}

pub(crate) async fn inspect(store: &Store) -> Result<Value> {
    let mut result = json!({});
    for (key, file) in [
        ("state", "state_5.sqlite"),
        ("history", "thread_history_1.sqlite"),
    ] {
        let path = store.root.join(file);
        if !path.is_file() {
            continue;
        }
        let mut connection = connect(&store.root, &path).await?;
        let names = sqlx::query("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").fetch_all(&mut connection).await?;
        let mut tables = json!({});
        for row in names {
            let name: String = row.try_get("name")?;
            let quoted = format!("\"{}\"", name.replace('"', "\"\""));
            let columns = sqlx::query(&format!("PRAGMA table_info({quoted})"))
                .fetch_all(&mut connection)
                .await?
                .iter()
                .filter_map(|row| row.try_get::<String, _>("name").ok())
                .collect::<Vec<_>>();
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {quoted}"))
                .fetch_one(&mut connection)
                .await?;
            tables[&name] = json!({"columns":columns,"row_count":count});
        }
        result[key] = json!({"path":path,"tables":tables});
    }
    Ok(result)
}
