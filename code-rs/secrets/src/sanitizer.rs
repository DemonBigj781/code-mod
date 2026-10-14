use regex::Regex;
use std::sync::LazyLock;

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk-[A-Za-z0-9_-]{16,}\b"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\bBearer\s+[A-Za-z0-9._~+\-/=]{12,}"));
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r#"(?i)\b([a-z0-9_]*(?:api[_-]?key|private[_-]?key|token|secret|password|passwd|authorization))\b(["']?\s*[:=]\s*)(["']?)[^\s,"';]+"#,
    )
});

static PRIVATE_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"(?s)-----BEGIN [^-\n]*PRIVATE KEY-----.*?-----END [^-\n]*PRIVATE KEY-----")
});
static URL_CREDENTIAL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/@:]+:[^\s/@]+@"));
static OTHER_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AIza[A-Za-z0-9_-]{20,})\b",
    )
});

/// Remove secret and keys from a String. This is done on best effort basis following some
/// well-known REGEX.
pub fn redact_secrets(input: String) -> String {
    let redacted = PRIVATE_KEY_REGEX.replace_all(&input, "[REDACTED_SECRET]");
    let redacted = URL_CREDENTIAL_REGEX.replace_all(&redacted, "$1[REDACTED_SECRET]@");
    let redacted = OTHER_TOKEN_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = OPENAI_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = BEARER_TOKEN_REGEX.replace_all(&redacted, "Bearer [REDACTED_SECRET]");
    let redacted = SECRET_ASSIGNMENT_REGEX.replace_all(&redacted, "$1$2$3[REDACTED_SECRET]");

    redacted.to_string()
}

/// Redact nested structured credentials before serializing tool records or exports.
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let normalized: String = key
                        .chars()
                        .filter(char::is_ascii_alphanumeric)
                        .flat_map(char::to_lowercase)
                        .collect();
                    let sensitive = matches!(
                        normalized.as_str(),
                        "authorization"
                            | "cookie"
                            | "cookies"
                            | "setcookie"
                            | "credential"
                            | "credentials"
                            | "encryptedcontent"
                            | "sessioncookie"
                    ) || [
                        "token",
                        "secret",
                        "apikey",
                        "password",
                        "passwd",
                        "privatekey",
                    ]
                    .iter()
                    .any(|suffix| normalized.ends_with(suffix));
                    (
                        key.clone(),
                        if sensitive {
                            Value::String("[REDACTED_SECRET]".into())
                        } else {
                            redact_json(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_json).collect()),
        Value::String(text) => Value::String(redact_secrets(text.clone())),
        _ => value.clone(),
    }
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        // Panic is ok thanks to `load_regex` test.
        Err(err) => panic!("invalid regex pattern `{pattern}`: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_regex() {
        // The goal of this test is just to compile all the regex to prevent the panic
        let _ = redact_secrets("secret".to_string());
    }

    #[test]
    fn redacts_short_nested_credentials_and_export_tokens() {
        let value = serde_json::json!({"nested": [{"api_key": "x", "refresh-token": "y"}], "text": "password=z ghp_abcdefghijklmnopqrstuv https://user:pass@example.test"});
        let result = redact_json(&value).to_string();
        assert!(!result.contains("abcdefghijklmnopqrstuv"));
        assert!(!result.contains("user:pass"));
        assert!(!result.contains("password=z"));
        assert_eq!(
            redact_json(&value)["nested"][0]["api_key"],
            "[REDACTED_SECRET]"
        );
    }

    #[test]
    fn redacts_prefixed_environment_keys_and_structured_credentials() {
        let value = serde_json::json!({"BRAVE_API_KEY":"tiny", "db_password":"short", "cookies":[{"name":"sid","value":"cookievalue"}], "command":"export SERVICE_API_KEY=secretvalue", "url":"https://example.test?access_token=hiddenvalue"});
        let redacted = redact_json(&value);
        assert_eq!(redacted["BRAVE_API_KEY"], "[REDACTED_SECRET]");
        assert_eq!(redacted["db_password"], "[REDACTED_SECRET]");
        assert_eq!(redacted["cookies"], "[REDACTED_SECRET]");
        assert!(!redacted.to_string().contains("secretvalue"));
        assert!(!redacted.to_string().contains("hiddenvalue"));
    }
}
