//! Structured diagnostic events and conservative sensitive-data redaction.

use std::time::Duration;

use serde_json::Value;

const REDACTED: &str = "[REDACTED]";

pub fn record_operation(
    component: &'static str,
    operation: &'static str,
    request_id: &str,
    provider_id: Option<&str>,
    outcome: &'static str,
    duration: Duration,
    retry_count: u32,
    error_code: Option<&str>,
) {
    tracing::info!(
        component,
        operation,
        request_id,
        provider_id = provider_id.unwrap_or(""),
        outcome,
        duration_ms = duration.as_millis() as u64,
        retry_count,
        error_code = error_code.unwrap_or(""),
        "backend operation completed"
    );
}

pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = if sensitive_key(key) {
                        Value::String(REDACTED.into())
                    } else {
                        redact_json(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_json).collect()),
        Value::String(value) => Value::String(redact_text(value)),
        other => other.clone(),
    }
}

pub fn redact_text(input: &str) -> String {
    let mut output = redact_url_queries(input);
    for marker in [
        "bearer ",
        "authorization=",
        "authorization:",
        "cookie=",
        "cookie:",
        "access_token=",
        "refresh_token=",
        "sms_code=",
        "verification_code=",
        "phone=",
    ] {
        output = redact_after_marker(&output, marker);
    }
    output
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "accesstoken"
            | "refreshtoken"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "phone"
            | "phonenumber"
            | "code"
            | "smscode"
            | "verificationcode"
            | "password"
            | "captchatoken"
            | "verificationtoken"
    )
}

fn redact_url_queries(input: &str) -> String {
    let mut output = input.to_owned();
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative_start) = lower[search_from..]
            .find("https://")
            .or_else(|| lower[search_from..].find("http://"))
        else {
            break;
        };
        let url_start = search_from + relative_start;
        let url_end = output[url_start..]
            .find(char::is_whitespace)
            .map(|offset| url_start + offset)
            .unwrap_or(output.len());
        let Some(query_offset) = output[url_start..url_end].find('?') else {
            search_from = url_end;
            continue;
        };
        let query_start = url_start + query_offset + 1;
        output.replace_range(query_start..url_end, REDACTED);
        search_from = query_start + REDACTED.len();
    }
    output
}

fn redact_after_marker(input: &str, marker: &str) -> String {
    let mut output = input.to_owned();
    let marker = marker.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative_start) = lower[search_from..].find(&marker) else {
            break;
        };
        let value_start = search_from + relative_start + marker.len();
        let value_end = output[value_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '&' | ';' | ',' | '}')
            })
            .map(|offset| value_start + offset)
            .unwrap_or(output.len());
        if value_start == value_end {
            search_from = value_start;
            continue;
        }
        output.replace_range(value_start..value_end, REDACTED);
        search_from = value_start + REDACTED.len();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_redaction_covers_credentials_and_signed_urls() {
        let source = json!({
            "providerId": "guangya",
            "accessToken": "access-secret",
            "nested": {
                "Authorization": "Bearer auth-secret",
                "phone": "13800138000",
                "code": "123456",
                "url": "https://example.invalid/video?id=1&sign=url-secret"
            }
        });
        let output = redact_json(&source).to_string();
        assert!(output.contains("guangya"));
        for secret in [
            "access-secret",
            "auth-secret",
            "13800138000",
            "123456",
            "url-secret",
        ] {
            assert!(!output.contains(secret));
        }
    }

    #[test]
    fn text_redaction_covers_headers_tokens_and_url_queries() {
        let output = redact_text(
            "Authorization:Bearer abc Cookie:session=def https://example.invalid/a?sign=ghi phone=13800138000",
        );
        for secret in ["abc", "session=def", "sign=ghi", "13800138000"] {
            assert!(!output.contains(secret));
        }
        assert!(output.matches(REDACTED).count() >= 4);
    }
}
