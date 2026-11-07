use std::fs;
use std::path::PathBuf;

#[test]
fn tui_draw_debug_logging_is_opt_in_and_forwarded() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", manifest_path.display())
    });
    let manifest: toml::Value = toml::from_str(&manifest_text).unwrap_or_else(|error| {
        panic!("failed to parse {}: {error}", manifest_path.display())
    });
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("Cargo.toml must define a features table");

    let defaults = features
        .get("default")
        .and_then(toml::Value::as_array)
        .expect("features.default must be an array");
    assert!(
        defaults
            .iter()
            .filter_map(toml::Value::as_str)
            .all(|feature| feature != "debug-logs"),
        "debug-logs must remain opt-in"
    );

    let debug_logs = features
        .get("debug-logs")
        .and_then(toml::Value::as_array)
        .expect("features.debug-logs must be an array");
    assert_eq!(
        debug_logs.as_slice(),
        [toml::Value::String("code-tui/debug-logs".to_owned())],
        "code-cli/debug-logs must forward to code-tui/debug-logs"
    );
}
