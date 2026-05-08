use std::time::Duration;

use reqwest::StatusCode;

use crate::foundation::Error;

const PROVIDER_ERROR_BODY_LIMIT: usize = 1024;
const REDACTED: &str = "[redacted]";

pub(super) fn client(provider: &'static str, timeout_secs: u64) -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if timeout_secs > 0 {
        builder = builder.timeout(Duration::from_secs(timeout_secs));
    }
    builder.build().unwrap_or_else(|error| {
        tracing::warn!(
            target: "forge.email",
            provider,
            error = %error,
            "failed to build HTTP email client; falling back to default client"
        );
        reqwest::Client::new()
    })
}

pub(super) async fn provider_error(
    provider: &'static str,
    status: StatusCode,
    response: reqwest::Response,
) -> Error {
    let body = response.text().await.unwrap_or_default();
    Error::message(format!(
        "{provider} API error ({status}): {}",
        sanitize_provider_error_body(&body)
    ))
}

fn sanitize_provider_error_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "<empty>".to_string();
    }

    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(body) {
        redact_sensitive_json(&mut value);
        return truncate_chars(&value.to_string(), PROVIDER_ERROR_BODY_LIMIT);
    }

    truncate_chars(&normalize_visible_text(body), PROVIDER_ERROR_BODY_LIMIT)
}

fn redact_sensitive_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if is_sensitive_key(key) {
                    *value = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_sensitive_json(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_json(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("authorization")
        || key.contains("password")
        || key.contains("secret")
        || key.contains("signature")
        || key.contains("token")
        || key.ends_with("key")
        || key.contains("_key")
        || key.contains("-key")
}

fn normalize_visible_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= limit {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::sanitize_provider_error_body;

    #[test]
    fn provider_error_body_redacts_sensitive_json_keys() {
        let body = r#"{"message":"bad","api_key":"secret","nested":{"token":"abc"}}"#;

        let sanitized = sanitize_provider_error_body(body);

        assert!(sanitized.contains("\"message\":\"bad\""));
        assert!(sanitized.contains("[redacted]"));
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("abc"));
    }

    #[test]
    fn provider_error_body_truncates_long_text() {
        let body = "x".repeat(1100);

        let sanitized = sanitize_provider_error_body(&body);

        assert!(sanitized.ends_with("..."));
        assert!(sanitized.len() < body.len());
    }
}
