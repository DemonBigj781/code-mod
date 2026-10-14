use super::*;
use serde_json::json;
use std::io::Write;
use std::path::Path;

fn settings(root: &Path) -> Settings {
    for name in ["code", "codex", "data/inbox"] {
        std::fs::create_dir_all(root.join(name)).expect("test operation succeeds");
    }
    Settings {
        stores: vec![
            Store {
                id: "code".into(),
                root: root.join("code"),
            },
            Store {
                id: "codex".into(),
                root: root.join("codex"),
            },
        ],
        data_dir: root.join("data"),
    }
}
fn request(value: Value) -> Request {
    serde_json::from_value(value).expect("test operation succeeds")
}
fn run(settings: &Settings, value: Value) -> Value {
    execute(settings.clone(), request(value)).expect("test operation succeeds")
}
fn rollout(root: &Path, id: &str) -> std::path::PathBuf {
    let path = root.join("sessions").join(format!("rollout-{id}.jsonl"));
    std::fs::create_dir_all(path.parent().expect("test operation succeeds"))
        .expect("test operation succeeds");
    let records = vec![
        json!({"type":"session_meta","payload":{"id":id,"cwd":"/workspace","timestamp":"2026-08-20T00:00:00Z"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-1"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"system","content":"PRIVATE SYSTEM"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","channel":"analysis","content":"PRIVATE THOUGHT"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Needle user token=abc"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Needle answer"}]}}),
        json!({"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"call-1","arguments":"{\"api_key\":\"tiny\",\"command\":\"echo needle\"}"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"needle result password=short"}}),
    ];
    let text = records.iter().map(|v| format!("{v}\n")).collect::<String>();
    std::fs::write(&path, text).expect("test operation succeeds");
    path
}

#[test]
fn rollout_search_pagination_turns_and_redaction() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    rollout(&settings.stores[0].root, "session-1");
    let stores = run(&settings, json!({"action":"list_stores"}));
    assert_eq!(stores["count"], 2);
    let sessions = run(
        &settings,
        json!({"action":"list_sessions","cwd_contains":"WORKSPACE"}),
    );
    assert_eq!(sessions["count"], 1);
    let result = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:session-1"}),
    );
    assert_eq!(
        result["items"]
            .as_array()
            .expect("test operation succeeds")
            .len(),
        4
    );
    for secret in ["PRIVATE", "token=abc", "tiny", "password=short"] {
        assert!(!result.to_string().contains(secret), "{secret}");
    }
    assert_eq!(result["items"][3]["tool_name"], "shell");
    let page = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:session-1","limit":1,"offset":1}),
    );
    assert_eq!(page["items"][0]["role"], "assistant");
    assert_eq!(page["next_offset"], 2);
    assert_eq!(page["truncated"], true);
    let turn = run(
        &settings,
        json!({"action":"get_turn","session_ref":"code:session-1","turn_id":"turn-1","include_tools":false}),
    );
    assert_eq!(
        turn["items"]
            .as_array()
            .expect("test operation succeeds")
            .len(),
        2
    );
    assert_eq!(
        run(
            &settings,
            json!({"action":"search_messages","query":"NEEDLE"})
        )["count"],
        4
    );
    assert_eq!(
        run(
            &settings,
            json!({"action":"search_messages","query":"needle","role":"user"})
        )["count"],
        1
    );
    let report = run(
        &settings,
        json!({"action":"inspect_store","store_id":"code","deep":true}),
    );
    assert!(!report.to_string().contains("Needle"));
}

#[test]
fn duplicate_store_ids_require_qualified_refs_and_history_fallback_works() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    for store in &settings.stores {
        rollout(&store.root, "shared");
    }
    assert_eq!(
        execute(
            settings.clone(),
            request(json!({"action":"get_session","session_ref":"shared"}))
        )
        .expect_err("request must fail")
        .code,
        "ambiguous_session"
    );
    std::fs::write(
        settings.stores[0].root.join("history.jsonl"),
        "{\"session_id\":\"orphan\",\"ts\":1700000000,\"text\":\"orphan prompt\"}\n",
    )
    .expect("test operation succeeds");
    let value = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:orphan"}),
    );
    assert_eq!(value["content_source"], "history");
    assert_eq!(value["items"][0]["text"], "orphan prompt");
    assert_eq!(
        execute(
            settings,
            request(json!({"action":"get_turn","session_ref":"code:orphan","turn_id":"missing"}))
        )
        .expect_err("request must fail")
        .code,
        "turn_unsupported"
    );
}

#[test]
fn request_limits_and_bad_arguments_are_not_silently_defaulted() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    for value in [
        json!({"action":"list_sessions","limit":0}),
        json!({"action":"list_sessions","limit":201}),
        json!({"action":"search_messages","query":"","limit":1}),
        json!({"action":"get_session","max_chars":200001}),
        json!({"action":"search_messages","query":"x","role":"system"}),
    ] {
        assert!(execute(settings.clone(), request(value)).is_err());
    }
    assert!(
        serde_json::from_value::<Request>(json!({"action":"list_sessions","offset":-1})).is_err()
    );
    assert!(
        serde_json::from_value::<Request>(json!({"action":"list_sessions","unknown":true}))
            .is_err()
    );
}

#[test]
fn native_backend_runs_from_an_async_tools_blocking_worker() {
    let dir = tempfile::tempdir().expect("temporary root");
    let settings = settings(dir.path());
    rollout(&settings.stores[0].root, "worker");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("outer tool runtime");
    let result = runtime.block_on(async {
        tokio::task::spawn_blocking(move || {
            execute(
                settings,
                request(json!({"action":"get_session","session_ref":"code:worker"})),
            )
        })
        .await
    });
    assert_eq!(
        result.expect("worker joins").expect("native query")["items"]
            .as_array()
            .expect("items")
            .len(),
        4
    );
}

#[test]
fn content_budget_counts_unicode_and_does_not_split_utf8() {
    let mut page = normalize::Page::new(0, 10, 3);
    assert!(!page.push(json!({"kind":"message","text":"界🙂xyz"})));
    let value = page.output();
    assert_eq!(value["truncated"], true);
    assert_eq!(value["next_offset"], 1);
    assert_eq!(value["items"][0]["text"], "界🙂x\n[TRUNCATED 2 CHARS]");
}

#[test]
fn retrieval_does_not_expand_a_redaction_marker_after_applying_the_budget() {
    let dir = tempfile::tempdir().expect("temporary root");
    let settings = settings(dir.path());
    rollout(&settings.stores[0].root, "budget");
    let full = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:budget"}),
    );
    let short = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:budget","max_chars":25}),
    );
    let expected: String = full["items"][0]["text"]
        .as_str()
        .expect("text")
        .chars()
        .take(25)
        .collect();
    let actual = short["items"][0]["text"].as_str().expect("shortened text");
    assert_eq!(actual.lines().next(), Some(expected.as_str()));
    assert_eq!(short["truncated"], true);
}

#[test]
fn malformed_metadata_is_not_copied_into_normalized_messages() {
    let dir = tempfile::tempdir().expect("temporary root");
    let settings = settings(dir.path());
    let path = rollout(&settings.stores[0].root, "metadata");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append fixture");
    writeln!(
        file,
        "{}",
        json!({"type":"turn_context","payload":{"turn_id":{"secret":"PRIVATE"}}})
    )
    .expect("write fixture");
    writeln!(file, "{}", json!({"type":"response_item","timestamp":{"secret":"PRIVATE"},"ordinal":{"secret":"PRIVATE"},"payload":{"type":"message","id":{"secret":"PRIVATE"},"role":"assistant","content":"visible"}})).expect("write fixture");
    drop(file);
    let value = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:metadata"}),
    );
    assert!(!value.to_string().contains("PRIVATE"));
    let item = value["items"]
        .as_array()
        .expect("items")
        .last()
        .expect("last item");
    assert_eq!(item["text"], "visible");
    assert_eq!(item["turn_id"], "turn-1");
    assert!(item["ordinal"].is_u64());
    assert!(item.get("item_id").is_none());
}

#[test]
fn hidden_metadata_is_respected_even_with_an_empty_top_level_channel() {
    assert!(normalize::private_message(
        &json!({"channel":"","metadata":{"channel":"analysis"}})
    ));
    assert!(normalize::private_message(&json!({"channel":"Analysis"})));
    assert!(normalize::private_message(
        &json!({"metadata":{"is_visually_hidden_from_conversation":true}})
    ));
    assert!(!normalize::private_message(
        &json!({"channel":"final","metadata":{"is_visually_hidden_from_conversation":false}})
    ));
    let redacted = code_secrets::redact_secrets(r#"text {"password":"short"}"#.to_owned());
    assert!(!redacted.contains("short"));
}

#[test]
fn sqlite_projection_wins_over_rollout_and_stays_readonly() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    let root = &settings.stores[0].root;
    rollout(root, "projected");
    let path = root.join("thread_history_1.sqlite");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test operation succeeds");
    runtime.block_on(async {
        let mut connection=sqlx::SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true)).await.expect("test operation succeeds");
        sqlx::query("CREATE TABLE thread_items(thread_id TEXT,turn_id TEXT,item_id TEXT,rollout_ordinal INTEGER,created_at_ms INTEGER,item_type TEXT,item_json TEXT)").execute(&mut connection).await.expect("test operation succeeds");
        for (i,payload) in [json!({"type":"agentMessage","text":"projected needle"}),json!({"id":"cmd","type":"commandExecution","command":"echo needle","aggregatedOutput":"token=abc"}),json!({"type":"reasoning","content":"PRIVATE"})].into_iter().enumerate() {
            sqlx::query("INSERT INTO thread_items VALUES(?,?,?,?,?,?,?)").bind("projected").bind("turn-1").bind(i.to_string()).bind(i as i64).bind(1700000000000i64).bind(payload["type"].as_str().expect("test operation succeeds")).bind(payload.to_string()).execute(&mut connection).await.expect("test operation succeeds");
        }
        connection.close().await.expect("test operation succeeds");
    });
    drop(runtime);
    let original = std::fs::read(&path).expect("test operation succeeds");
    let result = run(
        &settings,
        json!({"action":"get_session","session_ref":"code:projected"}),
    );
    assert_eq!(result["content_source"], "sqlite_projection");
    assert_eq!(
        result["items"]
            .as_array()
            .expect("test operation succeeds")
            .len(),
        3
    );
    assert_eq!(result["items"][0]["text"], "projected needle");
    assert!(!result.to_string().contains("PRIVATE"));
    assert!(!result.to_string().contains("token=abc"));
    assert_eq!(
        std::fs::read(path).expect("test operation succeeds"),
        original
    );
}

fn conversation() -> Value {
    json!({"id":"chat-1","title":"title password=x","create_time":1700000000.125,"update_time":1700000100.5,"is_archived":false,"current_node":"answer","mapping":{
        "root":{"parent":null,"message":{"author":{"role":"system"},"content":{"content_type":"text","parts":["PRIVATE"]}}},
        "user":{"parent":"root","message":{"author":{"role":"user"},"content":{"content_type":"text","parts":["needle user"]},"create_time":1700000000.125}},
        "old":{"parent":"user","message":{"author":{"role":"assistant"},"content":{"content_type":"text","parts":["alternative needle"]}}},
        "answer":{"parent":"user","message":{"author":{"role":"assistant"},"content":{"content_type":"text","parts":["selected needle"]}}},
        "thought":{"parent":"user","message":{"author":{"role":"assistant"},"channel":"analysis","content":{"content_type":"text","parts":["PRIVATE THOUGHT"]}}}
    }})
}

#[test]
fn export_import_persists_branches_without_overwriting_or_retaining_private_messages() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    let source = settings.data_dir.join("inbox/conversations.json");
    let original = serde_json::to_vec(&json!([conversation()])).expect("test operation succeeds");
    std::fs::write(&source, &original).expect("test operation succeeds");
    let args = json!({"action":"import_openai_export","file_name":"conversations.json","dataset_name":"test"});
    let result = run(&settings, args.clone());
    assert_eq!(result["item_count"], 3);
    assert_eq!(result["excluded_message_count"], 2);
    assert_eq!(
        std::fs::read(source).expect("test operation succeeds"),
        original
    );
    assert_eq!(
        execute(settings.clone(), request(args))
            .expect_err("request must fail")
            .code,
        "dataset_exists"
    );
    let sessions = run(
        &settings,
        json!({"action":"list_sessions","source_kind":"openai_export"}),
    );
    assert_eq!(sessions["count"], 1);
    assert_eq!(sessions["items"][0]["created_at_ms"], 1700000000125i64);
    let page = run(
        &settings,
        json!({"action":"get_session","session_ref":"openai-test:chat-1"}),
    );
    assert_eq!(page["content_source"], "openai_export");
    let items = page["items"].as_array().expect("test operation succeeds");
    assert_eq!(
        items
            .iter()
            .find(|item| item["item_id"] == "old")
            .expect("test operation succeeds")["is_active_branch"],
        false
    );
    assert_eq!(
        run(
            &settings,
            json!({"action":"get_turn","session_ref":"openai-test:chat-1","turn_id":"user"})
        )["items"]
            .as_array()
            .expect("test operation succeeds")
            .len(),
        3
    );
    for entry in walkdir::WalkDir::new(settings.data_dir.join("datasets")) {
        let entry = entry.expect("test operation succeeds");
        if entry.file_type().is_file() {
            assert!(
                !std::fs::read_to_string(entry.path())
                    .expect("test operation succeeds")
                    .contains("PRIVATE")
            );
        }
    }
}

#[test]
fn zip_import_reads_only_conversations_and_rejects_path_traversal() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    for (file, member, ok) in [
        ("good.zip", "export/conversations-001.json", true),
        ("bad.zip", "../conversations.json", false),
    ] {
        let output = std::fs::File::create(settings.data_dir.join("inbox").join(file))
            .expect("test operation succeeds");
        let mut zip = zip::ZipWriter::new(output);
        zip.start_file(member, zip::write::SimpleFileOptions::default())
            .expect("test operation succeeds");
        zip.write_all(
            serde_json::to_string(&json!([conversation()]))
                .expect("test operation succeeds")
                .as_bytes(),
        )
        .expect("test operation succeeds");
        zip.start_file("../asset.txt", zip::write::SimpleFileOptions::default())
            .expect("test operation succeeds");
        zip.write_all(b"ignored asset")
            .expect("test operation succeeds");
        zip.finish().expect("test operation succeeds");
        let result = execute(
            settings.clone(),
            request(
                json!({"action":"import_openai_export","file_name":file,"dataset_name":if ok{"good"}else{"bad"}}),
            ),
        );
        assert_eq!(result.is_ok(), ok);
    }
    assert!(!settings.data_dir.join("datasets/openai-bad").exists());
    assert!(execute(settings,request(json!({"action":"import_openai_export","file_name":"../outside.json","dataset_name":"no"}))).is_err());
}

#[test]
fn cycles_and_duplicate_conversations_do_not_publish_a_partial_dataset() {
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    let mut cyclic = conversation();
    cyclic["mapping"]["root"]["parent"] = json!("answer");
    for (name, value) in [
        ("cycle", json!([cyclic])),
        ("duplicate", json!([conversation(), conversation()])),
    ] {
        std::fs::write(
            settings.data_dir.join("inbox/conversations.json"),
            value.to_string(),
        )
        .expect("test operation succeeds");
        assert!(execute(settings.clone(),request(json!({"action":"import_openai_export","file_name":"conversations.json","dataset_name":name}))).is_err());
        assert!(
            !settings
                .data_dir
                .join("datasets")
                .join(format!("openai-{name}"))
                .exists()
        );
    }
}

#[test]
#[cfg(unix)]
fn symlinked_sources_cannot_escape_a_store_or_import_inbox() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().expect("test operation succeeds");
    let settings = settings(dir.path());
    let outside = dir.path().join("outside.json");
    std::fs::write(&outside, "[]").expect("test operation succeeds");
    symlink(&outside, settings.data_dir.join("inbox/link.json")).expect("test operation succeeds");
    assert!(execute(settings.clone(),request(json!({"action":"import_openai_export","file_name":"link.json","dataset_name":"unsafe"}))).is_err());
    assert!(files::allowed_path(&settings.stores[0].root, &outside).is_err());
}

#[test]
fn archived_sources_and_opaque_identifiers_remain_discoverable() {
    let dir = tempfile::tempdir().expect("temporary root");
    let settings = settings(dir.path());
    let root = &settings.stores[0].root;
    let first = rollout(root, "token=first");
    rollout(root, "token=second");
    std::fs::create_dir(root.join("archived_sessions")).expect("archive directory");
    std::fs::rename(first, root.join("archived_sessions/rollout-archived.jsonl"))
        .expect("archive fixture");
    let page = run(&settings, json!({"action":"list_sessions","archived":true}));
    assert_eq!(page["count"], 1);
    assert_eq!(page["items"][0]["ref"], "code:token=first");
    let sources = run(&settings, json!({"action":"list_stores"}));
    assert!(
        sources["stores"][0]["sources"]
            .as_array()
            .expect("sources")
            .contains(&json!("archived_sessions/**/*.jsonl"))
    );
    assert_eq!(
        run(&settings, json!({"action":"list_sessions"}))["total"],
        2
    );
    assert_eq!(
        run(
            &settings,
            json!({"action":"get_session","session_ref":"code:token=first"})
        )["session"]["session_id"],
        "token=first"
    );
}

#[test]
fn imported_tool_scalar_results_keep_their_json_type() {
    let dir = tempfile::tempdir().expect("temporary root");
    let settings = settings(dir.path());
    let mut export = conversation();
    export["mapping"]["call"] = json!({"parent":"user","message":{"author":{"role":"assistant"},"recipient":"python","content":{"content_type":"text","parts":["42"]}}});
    export["mapping"]["result"] = json!({"parent":"call","message":{"author":{"role":"tool"},"content":{"content_type":"text","parts":["42"]}}});
    export["current_node"] = json!("result");
    std::fs::write(
        settings.data_dir.join("inbox/conversations.json"),
        json!([export]).to_string(),
    )
    .expect("export fixture");
    run(
        &settings,
        json!({"action":"import_openai_export","file_name":"conversations.json","dataset_name":"scalars"}),
    );
    let page = run(
        &settings,
        json!({"action":"get_session","session_ref":"openai-scalars:chat-1"}),
    );
    let result = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["item_id"] == "result")
        .expect("tool result");
    assert_eq!(result["result"], 42);
    assert_eq!(result["tool_name"], "python");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in walkdir::WalkDir::new(settings.data_dir.join("datasets")) {
            let entry = entry.expect("dataset entry");
            assert_eq!(
                entry.metadata().expect("metadata").permissions().mode() & 0o077,
                0
            );
        }
    }
}
