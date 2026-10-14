use code_session_history::{Request, Settings, Store};
use serde_json::{Value, json};

fn call(settings: &Settings, request: Value) -> Value {
    let request: Request = serde_json::from_value(request).expect("valid request");
    code_session_history::execute(settings.clone(), request).expect("native history operation")
}

#[test]
fn native_history_operations_work_without_an_mcp_server() {
    let root = tempfile::tempdir().expect("isolated store");
    let sessions = root.path().join("sessions");
    let data = root.path().join("data");
    std::fs::create_dir(&sessions).expect("sessions");
    std::fs::create_dir_all(data.join("inbox")).expect("inbox");
    let records = [
        json!({"type":"session_meta","payload":{"id":"demo","cwd":"/fixture","base_instructions":{"text":"PRIVATE"}}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-1"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"developer","content":"PRIVATE"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":"Needle password=tiny"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","channel":"analysis","content":"PRIVATE"}}),
    ];
    let bytes = records
        .iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
    let rollout = sessions.join("rollout-demo.jsonl");
    std::fs::write(&rollout, &bytes).expect("fixture rollout");
    let settings = Settings {
        stores: vec![Store {
            id: "fixture".into(),
            root: root.path().to_path_buf(),
        }],
        data_dir: data.clone(),
    };
    assert_eq!(call(&settings, json!({"action":"list_stores"}))["count"], 1);
    assert_eq!(
        call(&settings, json!({"action":"list_sessions"}))["items"][0]["ref"],
        "fixture:demo"
    );
    assert_eq!(
        call(
            &settings,
            json!({"action":"search_messages","query":"NEEDLE"})
        )["count"],
        1
    );
    let page = call(
        &settings,
        json!({"action":"get_session","session_ref":"fixture:demo"}),
    );
    assert_eq!(page["items"].as_array().expect("items").len(), 1);
    assert!(!page.to_string().contains("PRIVATE"));
    assert!(!page.to_string().contains("tiny"));
    assert_eq!(
        call(
            &settings,
            json!({"action":"get_turn","session_ref":"fixture:demo","turn_id":"turn-1"})
        )["items"],
        page["items"]
    );
    let inspection = call(
        &settings,
        json!({"action":"inspect_store","store_id":"fixture","deep":true}),
    );
    assert_eq!(inspection["rollout_count"], 1);
    assert!(!inspection.to_string().contains("Needle"));
    assert_eq!(
        std::fs::read_to_string(&rollout).expect("original rollout"),
        bytes
    );

    let export = json!([{"id":"imported","title":"Imported test","mapping":{"user":{"parent":null,"message":{"author":{"role":"user"},"content":{"content_type":"text","parts":["imported message"]}}}},"current_node":"user"}]);
    let input = data.join("inbox/conversations.json");
    std::fs::write(&input, export.to_string()).expect("export fixture");
    let result = call(
        &settings,
        json!({"action":"import_openai_export","file_name":"conversations.json","dataset_name":"smoke"}),
    );
    assert_eq!(result["item_count"], 1);
    assert_eq!(
        call(
            &settings,
            json!({"action":"get_session","session_ref":"openai-smoke:imported"})
        )["items"][0]["text"],
        "imported message"
    );
    assert_eq!(
        std::fs::read_to_string(input).expect("original export"),
        export.to_string()
    );
}
