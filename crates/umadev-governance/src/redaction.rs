//! Shared secret redaction for persisted and user-visible diagnostics.

use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::Value;

const REDACTED: &str = "[redacted]";

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_pagination_key(key: &str) -> bool {
    matches!(
        key,
        "cursor"
            | "nextcursor"
            | "pagecursor"
            | "paginationcursor"
            | "cursortoken"
            | "pagetoken"
            | "nextpagetoken"
            | "paginationtoken"
            | "continuationtoken"
            | "resumetoken"
    )
}

fn is_token_metric_key(key: &str) -> bool {
    matches!(
        key,
        "inputtokens"
            | "outputtokens"
            | "totaltokens"
            | "cachedtokens"
            | "reasoningtokens"
            | "maxtokens"
            | "tokencount"
            | "inputtokencount"
            | "outputtokencount"
            | "tokenusage"
            | "tokenbudget"
    )
}

fn is_sensitive_key(key: &str) -> bool {
    let key = normalized_key(key);
    if is_pagination_key(&key) || is_token_metric_key(&key) {
        return false;
    }
    if matches!(
        key.as_str(),
        "env" | "environment" | "environmentvariables" | "headers" | "httpheaders"
    ) {
        return true;
    }
    if matches!(
        key.as_str(),
        "token"
            | "authorization"
            | "proxyauthorization"
            | "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "idtoken"
            | "sessiontoken"
            | "apitoken"
            | "password"
            | "passwd"
            | "pwd"
            | "passphrase"
            | "secret"
            | "clientsecret"
            | "secretkey"
            | "credential"
            | "credentials"
            | "cookie"
            | "setcookie"
            | "privatekey"
            | "privatekeypem"
    ) {
        return true;
    }
    [
        "token",
        "authorization",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "idtoken",
        "sessiontoken",
        "apitoken",
        "password",
        "passphrase",
        "clientsecret",
        "secret",
        "secretkey",
        "secretaccesskey",
        "credential",
        "privatekey",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

fn assignment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(?P<prefix>(?:authorization|proxy[-_]?authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|auth[-_]?token|id[-_]?token|session[-_]?token|client[-_]?secret|secret[-_]?key|password|passwd|passphrase|private[-_]?key)\s*[\"']?\s*[:=]\s*)[^\r\n]+"#,
        )
        .expect("static sensitive-assignment regex is valid")
    })
}

fn pem_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?is)-----BEGIN [^-\r\n]*PRIVATE KEY-----.*?(?:-----END [^-\r\n]*PRIVATE KEY-----|\z)",
        )
        .expect("static private-key regex is valid")
    })
}

fn bearer_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?P<prefix>\bbearer\s+)(?P<value>[A-Za-z0-9._~+/=-]{8,})")
            .expect("static bearer regex is valid")
    })
}

fn prefixed_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:ghp_|github_pat_|sk-|xai-)[A-Za-z0-9._~+/=-]{8,}")
            .expect("static token-prefix regex is valid")
    })
}

fn token_assignment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(?P<prefix>(?P<key>[A-Za-z][A-Za-z0-9_-]*token)\s*[\"']?\s*[:=]\s*)[^\r\n]+"#,
        )
        .expect("static token-assignment regex is valid")
    })
}

/// Redact common credential assignments, bearer values, private keys, and token prefixes.
#[must_use]
pub fn redact_text(text: &str) -> String {
    let without_pem = pem_regex().replace_all(text, "[redacted private key]");
    let without_assignments = assignment_regex().replace_all(&without_pem, "${prefix}[redacted]");
    let without_tokens =
        token_assignment_regex().replace_all(&without_assignments, |captures: &Captures<'_>| {
            let key = captures.name("key").map_or("", |value| value.as_str());
            let normalized = normalized_key(key);
            if is_pagination_key(&normalized) || is_token_metric_key(&normalized) {
                captures
                    .get(0)
                    .map_or("", |value| value.as_str())
                    .to_string()
            } else {
                format!(
                    "{}[redacted]",
                    captures
                        .name("prefix")
                        .map_or("token=", |value| value.as_str())
                )
            }
        });
    let without_bearer = bearer_regex().replace_all(&without_tokens, |captures: &Captures<'_>| {
        let value = captures.name("value").map_or("", |value| value.as_str());
        if matches!(
            value.to_ascii_lowercase().as_str(),
            "authentication" | "credentials" | "placeholder" | "exampletoken"
        ) {
            captures
                .get(0)
                .map_or("", |value| value.as_str())
                .to_string()
        } else {
            format!(
                "{}[redacted]",
                captures
                    .name("prefix")
                    .map_or("Bearer ", |value| value.as_str())
            )
        }
    });
    prefixed_token_regex()
        .replace_all(&without_bearer, REDACTED)
        .into_owned()
}

/// Recursively redact sensitive JSON keys and string values.
#[must_use]
pub fn redact_json(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, Value::String(REDACTED.to_string()))
                    } else {
                        (key, redact_json(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_json).collect()),
        Value::String(text) => Value::String(redact_text(&text)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_without_hiding_metrics_or_pagination() {
        let text = "api_key=sk-live-secret-value\ninput_tokens=42\npage_token=page-4";
        let redacted = redact_text(text);
        assert!(!redacted.contains("sk-live-secret-value"));
        assert!(redacted.contains("input_tokens=42"));
        assert!(redacted.contains("page_token=page-4"));
    }

    #[test]
    fn redacts_nested_json_by_key_and_value_shape() {
        let value = serde_json::json!({
            "headers": {"Authorization": "Bearer live-secret-token"},
            "message": "use ghp_1234567890abcdef",
            "token_usage": 99
        });
        let redacted = redact_json(value);
        assert_eq!(redacted["headers"], REDACTED);
        assert_eq!(redacted["message"], "use [redacted]");
        assert_eq!(redacted["token_usage"], 99);
    }
}
