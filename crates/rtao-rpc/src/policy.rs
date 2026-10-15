// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Terminal gateway refusals and bounded upstream diagnostics.

use std::{
    fmt,
    time::{Duration, SystemTime},
};

use http::HeaderMap;
use serde_json::Value;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::generated::py_rpc;

/// The gateway contract that caused a terminal refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// An HTTP 403 or 429 response during the WebSocket upgrade.
    Handshake {
        /// The upgrade response's HTTP status.
        status: u16,
    },
    /// An explicitly recognized JSON-RPC traffic-policy error.
    JsonRpc {
        /// The provider-defined JSON-RPC error code.
        code: i64,
    },
    /// An RFC 6455 policy-violation close.
    Close1008,
}

/// A terminal access or traffic-policy refusal, retaining Python's diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRefusal {
    /// The contract used to recognize this refusal.
    pub kind: RefusalKind,
    /// The bounded diagnostic with the pinned Python wording.
    pub message: String,
    /// HTTP 403/429, WebSocket 1008, or no status for JSON-RPC errors.
    pub status_code: Option<u16>,
    /// The sanitized raw retry value that Python exposes, in seconds or HTTP-date.
    pub retry_after: Option<String>,
    /// The sanitized gateway policy identifier, when supplied.
    pub policy: Option<String>,
    /// The sanitized gateway refusal reason, when supplied.
    pub reason: Option<String>,
    // Python recognizes these keys but does not expose them as retry_after.
    retry_after_ms: Option<String>,
}

impl PolicyRefusal {
    /// Parse delta-seconds, millisecond metadata, or an HTTP-date.
    ///
    /// Invalid or negative values return `None`; elapsed HTTP-dates return zero.
    pub fn retry_after_duration(&self) -> Option<Duration> {
        self.retry_after_duration_at(SystemTime::now())
    }

    fn retry_after_duration_at(&self, now: SystemTime) -> Option<Duration> {
        if let Some(raw) = &self.retry_after {
            if let Some(duration) = numeric_duration(raw, 1.0) {
                return Some(duration);
            }
            if let Ok(deadline) = httpdate::parse_http_date(raw) {
                return Some(deadline.duration_since(now).unwrap_or(Duration::ZERO));
            }
        }
        self.retry_after_ms
            .as_deref()
            .and_then(|raw| numeric_duration(raw, 1000.0))
    }
}

impl fmt::Display for PolicyRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PolicyRefusal {}

fn numeric_duration(raw: &str, denominator: f64) -> Option<Duration> {
    if let Ok(value) = raw.trim().parse::<u64>() {
        return Some(if denominator == 1.0 {
            Duration::from_secs(value)
        } else {
            Duration::from_millis(value)
        });
    }
    let value = raw.trim().parse::<f64>().ok()? / denominator;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(value).ok()
}

/// Recognize only the pinned JSON-RPC traffic-policy contract.
///
/// The code must be an integer in the extracted set, and either its message
/// matches exactly after trimming and case folding or its object data contains
/// an extracted policy metadata key. State-discarded errors take precedence.
pub fn classify_rpc_error(error: &Value) -> Option<PolicyRefusal> {
    let object = error.as_object()?;
    let message = object.get("message").map(python_string).unwrap_or_default();
    let data = object.get("data");
    if message.contains(py_rpc::STATE_DISCARDED_NEEDLE)
        || data
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains(py_rpc::STATE_DISCARDED_NEEDLE))
    {
        return None;
    }
    let code = object.get("code")?.as_i64()?;
    if !py_rpc::POLICY_RPC_ERROR_CODES.contains(&code) {
        return None;
    }
    let normalized = policy_casefold(&message);
    let known_message = py_rpc::POLICY_RPC_ERROR_MESSAGES
        .iter()
        .any(|(candidate, messages)| *candidate == code && messages.contains(&normalized.as_str()));
    let metadata = data.and_then(Value::as_object);
    let known_metadata = metadata.is_some_and(|object| {
        py_rpc::POLICY_RPC_METADATA_KEYS
            .iter()
            .any(|key| object.contains_key(*key))
    });
    if !known_message && !known_metadata {
        return None;
    }
    let field = |name: &str| metadata.and_then(|object| object.get(name));
    // Python's `a or b` is deliberate here: false, zero, and empty containers
    // select the camelCase fallback; a false fallback is still preserved.
    let retry_after = field("retry_after")
        .filter(|value| python_truth(value))
        .or_else(|| field("retryAfter"));
    let retry_after_ms = field("retry_after_ms")
        .filter(|value| !value.is_null())
        .or_else(|| field("retryAfterMs"));
    let sanitized = |value: Option<&Value>| {
        value
            .filter(|value| !value.is_null())
            .map(|value| safe_diagnostic(&python_string(value), py_rpc::POLICY_METADATA_LIMIT))
    };
    Some(PolicyRefusal {
        kind: RefusalKind::JsonRpc { code },
        message: format!(
            "RPC endpoint refused the request under its traffic policy: {}",
            safe_diagnostic(&format_rpc_error(error), py_rpc::POLICY_DETAIL_LIMIT)
        ),
        status_code: None,
        retry_after: sanitized(retry_after),
        policy: sanitized(field("policy")),
        reason: sanitized(field("reason")),
        retry_after_ms: sanitized(retry_after_ms),
    })
}

/// Classify HTTP 403/429 upgrade failures, retaining the first duplicate header.
pub fn classify_handshake(
    status: u16,
    headers: &HeaderMap,
    body: Option<&[u8]>,
) -> Option<PolicyRefusal> {
    let mut message = match status {
        429 => "RPC endpoint rate limited this source (HTTP 429)".to_owned(),
        403 => "RPC endpoint rejected this source under its access policy (HTTP 403)".to_owned(),
        _ => return None,
    };
    let detail = response_detail(body.unwrap_or_default());
    if !detail.is_empty() {
        message.push_str(": ");
        message.push_str(&detail);
    }
    let policy = response_header(headers, "x-ratelimit-policy");
    let reason = response_header(headers, "x-ratelimit-reason");
    let retry_after = response_header(headers, "retry-after")
        .or_else(|| response_header(headers, "x-ratelimit-reset"));
    let qualifiers: Vec<_> = [
        policy.as_ref().map(|value| format!("policy={value}")),
        reason.as_ref().map(|value| format!("reason={value}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !qualifiers.is_empty() {
        message.push_str(" (");
        message.push_str(&qualifiers.join(", "));
        message.push(')');
    }
    Some(PolicyRefusal {
        kind: RefusalKind::Handshake { status },
        message,
        status_code: Some(status),
        retry_after,
        policy,
        reason,
        retry_after_ms: None,
    })
}

/// Classify an RFC 6455 close with code 1008 as a terminal policy refusal.
pub fn classify_close(code: u16, reason: &str) -> Option<PolicyRefusal> {
    if code != 1008 {
        return None;
    }
    let detail = safe_diagnostic(reason, py_rpc::POLICY_DETAIL_LIMIT);
    let mut message =
        "RPC endpoint closed the connection for a policy violation (WebSocket 1008)".to_owned();
    if !detail.is_empty() {
        message.push_str(": ");
        message.push_str(&detail);
    }
    Some(PolicyRefusal {
        kind: RefusalKind::Close1008,
        message,
        status_code: Some(code),
        retry_after: None,
        policy: None,
        reason: (!detail.is_empty()).then_some(detail),
        retry_after_ms: None,
    })
}

/// Remove terminal controls, collapse Unicode whitespace, and bound characters.
pub fn safe_diagnostic(value: &str, limit: usize) -> String {
    let clean: String = value
        .chars()
        .map(|character| {
            let ordinal = u32::from(character);
            if ordinal < 32
                || (127..=159).contains(&ordinal)
                || get_general_category(character) == GeneralCategory::Format
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    clean
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

/// Render the first 4096 response bytes as a bounded Python-style diagnostic.
pub fn response_detail(body: &[u8]) -> String {
    if body.is_empty() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&body[..body.len().min(py_rpc::POLICY_BODY_LIMIT)]);
    let text = text.trim_matches(python_whitespace);
    let detail = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => {
            let chosen = object
                .get("error")
                .filter(|value| python_truth(value))
                .or_else(|| object.get("message").filter(|value| python_truth(value)));
            chosen
                .map(python_string)
                .unwrap_or_else(|| python_string(&Value::Object(object)))
        }
        Ok(value) => python_string(&value),
        Err(_) => text.to_owned(),
    };
    safe_diagnostic(&detail, py_rpc::POLICY_DETAIL_LIMIT)
}

fn response_header(headers: &HeaderMap, name: &'static str) -> Option<String> {
    let header = headers.get_all(name).iter().next()?;
    let value = String::from_utf8_lossy(header.as_bytes());
    let clean = safe_diagnostic(&value, py_rpc::POLICY_METADATA_LIMIT);
    (!clean.is_empty()).then_some(clean)
}

/// Render an endpoint without user information, path, query, or fragment secrets.
pub fn safe_endpoint(endpoint: &str) -> String {
    let fallback = || "the configured RPC endpoint".to_owned();
    let Ok(parsed) = url::Url::parse(endpoint) else {
        return fallback();
    };
    let Some(host) = parsed.host_str() else {
        return fallback();
    };
    // Url normalizes an explicit default port away, unlike Python urlsplit.
    let authority = endpoint
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    let authority = authority.map(|value| value.rsplit('@').next().unwrap_or_default());
    let explicit_port = authority.and_then(|authority| {
        if authority.starts_with('[') {
            authority
                .split_once(']')
                .and_then(|(_, tail)| tail.strip_prefix(':'))
        } else {
            authority.rsplit_once(':').map(|(_, port)| port)
        }
    });
    let port = if let Some(port) = explicit_port {
        if port.is_empty() {
            None
        } else {
            let Ok(port) = port.parse::<u16>() else {
                return fallback();
            };
            Some(port)
        }
    } else {
        parsed.port()
    };
    let mut result = format!("{}://{host}", parsed.scheme());
    if let Some(port) = port {
        result.push(':');
        result.push_str(&port.to_string());
    }
    result
}

/// Format a JSON-RPC error object using Python's `format_rpc_error` rules.
pub(crate) fn format_rpc_error(error: &Value) -> String {
    let Some(object) = error.as_object() else {
        return python_string(error);
    };
    let message = object
        .get("message")
        .filter(|value| python_truth(value))
        .map(python_string)
        .unwrap_or_else(|| "Request failed".to_owned());
    match object.get("data").filter(|value| !value.is_null()) {
        Some(data) => format!("{message}: {}", python_string(data)),
        None => message,
    }
}

fn python_truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn python_whitespace(character: char) -> bool {
    // Python includes the four ASCII information separators in str.isspace().
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

fn policy_casefold(message: &str) -> String {
    // The extracted literals are ASCII. These are all non-ASCII full casefold
    // expansions into ASCII; the remaining folds either match lowercase or
    // contain a non-ASCII character and therefore cannot match those literals.
    let mut folded = String::new();
    for character in message.trim_matches(python_whitespace).chars() {
        match character {
            '\u{df}' | '\u{1e9e}' => folded.push_str("ss"),
            '\u{17f}' => folded.push('s'),
            '\u{fb00}' => folded.push_str("ff"),
            '\u{fb01}' => folded.push_str("fi"),
            '\u{fb02}' => folded.push_str("fl"),
            '\u{fb03}' => folded.push_str("ffi"),
            '\u{fb04}' => folded.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => folded.push_str("st"),
            character => folded.extend(character.to_lowercase()),
        }
    }
    folded
}

fn python_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => python_repr(value),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
        Value::Number(value) => python_number(value),
        Value::String(value) => python_string_repr(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_string_repr(key), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_number(value: &serde_json::Number) -> String {
    let raw = value.to_string();
    if !raw.contains(['.', 'e', 'E']) {
        return if raw == "-0" { "0".to_owned() } else { raw };
    }
    let Ok(value) = raw.parse::<f64>() else {
        return raw;
    };
    if value == f64::INFINITY {
        return "inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".to_owned();
    }
    let scientific = format!("{value:e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return raw;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return raw;
    };
    if !(-4..16).contains(&exponent) {
        format!("{mantissa}e{exponent:+03}")
    } else {
        let mut fixed = value.to_string();
        if !fixed.contains('.') {
            fixed.push_str(".0");
        }
        fixed
    }
}

/// Format a string with Python's diagnostic `repr` quoting and escaping.
pub fn python_string_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::new();
    result.push(quote);
    for character in value.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character == quote => {
                result.push('\\');
                result.push(character);
            }
            character if !python_printable(character) => {
                let ordinal = u32::from(character);
                result.push_str(&if ordinal <= 0xff {
                    format!("\\x{ordinal:02x}")
                } else if ordinal <= 0xffff {
                    format!("\\u{ordinal:04x}")
                } else {
                    format!("\\U{ordinal:08x}")
                });
            }
            character => result.push(character),
        }
    }
    result.push(quote);
    result
}

fn python_printable(character: char) -> bool {
    character == ' '
        || !matches!(
            get_general_category(character),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::PrivateUse
                | GeneralCategory::Surrogate
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::SpaceSeparator
        )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    // pyt: sdk/python/tests/unit/test_rpc.py::test_response_detail_shapes
    #[test]
    fn response_detail_shapes() {
        for (body, expected) in [
            (b"".as_slice(), ""),
            (
                b"temporarily unavailable".as_slice(),
                "temporarily unavailable",
            ),
            (b"\"busy\"".as_slice(), "busy"),
            (b"{\"message\":\"try later\"}".as_slice(), "try later"),
            (b"{\"other\":\"detail\"}".as_slice(), "{'other': 'detail'}"),
        ] {
            assert_eq!(response_detail(body), expected);
        }
    }

    // pyt: sdk/python/tests/unit/test_rpc.py::test_load_shed_policy_supports_camel_case_retry_metadata
    #[test]
    fn load_shed_policy_supports_camel_case_retry_metadata() {
        let error = classify_rpc_error(&json!({"code": -32005, "message": "upstream capacity exceeded", "data": {"retryAfter": 15}})).expect("policy metadata is recognized");
        assert_eq!(error.retry_after.as_deref(), Some("15"));
        assert_eq!(error.policy, None);
    }

    // pyt: sdk/python/tests/unit/test_rpc.py::test_provider_defined_code_without_policy_contract_is_not_terminal
    #[test]
    fn provider_defined_code_without_policy_contract_is_not_terminal() {
        for code in [-32004, -32005] {
            assert!(classify_rpc_error(&json!({"code": code, "message": "provider-specific server error", "data": {"detail": "not a traffic policy"}})).is_none());
        }
    }

    // pyt: sdk/python/tests/unit/test_rpc.py::test_known_storage_work_policy_without_metadata_remains_terminal
    #[test]
    fn known_storage_work_policy_without_metadata_remains_terminal() {
        assert!(
            classify_rpc_error(
                &json!({"code": -32004, "message": "Storage work rate limit exceeded"})
            )
            .is_some()
        );
    }

    // pyt: sdk/python/tests/unit/test_rpc.py::test_json_rpc_policy_diagnostics_strip_terminal_controls
    #[test]
    fn json_rpc_policy_diagnostics_strip_terminal_controls() {
        let error = classify_rpc_error(&json!({"code": -32004, "message": "slow\u{1b}[2Jdown", "data": {"policy": "public\u{1b}]8;;https://evil.example\u{7}rpc", "reason": "weighted\0rate"}})).expect("policy is recognized");
        assert!(!error.message.contains('\u{1b}'));
        assert!(!error.policy.expect("policy retained").contains('\u{1b}'));
        assert!(!error.reason.expect("reason retained").contains('\0'));
    }

    #[test]
    fn policy_code_and_metadata_classification_are_strict() {
        for code in [
            json!("-32004"),
            json!(-32004.0),
            json!(true),
            json!(-32003),
            json!(0),
        ] {
            assert!(classify_rpc_error(&json!({"code": code, "message": "storage work rate limit exceeded", "data": {"policy": "gateway"}})).is_none());
        }
        for code in py_rpc::POLICY_RPC_ERROR_CODES {
            for key in py_rpc::POLICY_RPC_METADATA_KEYS {
                let mut data = serde_json::Map::new();
                data.insert((*key).to_owned(), Value::Null);
                assert!(
                    classify_rpc_error(
                        &json!({"code": code, "message": "unrecognized", "data": data})
                    )
                    .is_some()
                );
            }
        }
        for message in [
            " STORAGE WORK RATE LIMIT EXCEEDED\n",
            "\u{17f}torage work rate limit exceeded",
            "\u{1c}\u{fb06}orage wor\u{212a} rate limit exceeded\u{1f}",
        ] {
            assert!(classify_rpc_error(&json!({"code": -32004, "message": message})).is_some());
        }
        assert!(
            classify_rpc_error(
                &json!({"code": -32005, "message": "storage work rate limit exceeded"})
            )
            .is_none()
        );
        for data in [
            Value::Null,
            json!("policy"),
            json!(["policy"]),
            json!({"Policy": "gateway"}),
        ] {
            assert!(
                classify_rpc_error(&json!({"code": -32004, "message": "other", "data": data}))
                    .is_none()
            );
        }
    }

    #[test]
    fn state_discarded_precedes_policy_contract() {
        for error in [
            json!({"code": -32004, "message": "State already discarded for 0x123", "data": {"policy": "gateway"}}),
            json!({"code": -32004, "message": "storage work rate limit exceeded", "data": "State already discarded for 0x123"}),
        ] {
            assert!(classify_rpc_error(&error).is_none());
        }
    }

    #[test]
    fn python_truthiness_and_raw_values_are_preserved() {
        let refusal = classify_rpc_error(&json!({"code": -32004, "message": "", "data": {"retry_after": 0, "retryAfter": 15, "policy": false, "reason": ""}})).expect("metadata recognized");
        assert_eq!(refusal.retry_after.as_deref(), Some("15"));
        assert_eq!(refusal.policy.as_deref(), Some("False"));
        assert_eq!(refusal.reason.as_deref(), Some(""));
        assert!(refusal.message.contains("Request failed:"));
        let zero = classify_rpc_error(&json!({"code": -32005, "data": {"retry_after": 0}}))
            .expect("metadata recognized");
        assert_eq!(zero.retry_after, None);
        let zero_fallback = classify_rpc_error(&json!({"code": -32005, "data": {"retryAfter": 0}}))
            .expect("metadata recognized");
        assert_eq!(zero_fallback.retry_after.as_deref(), Some("0"));
    }

    #[test]
    fn diagnostic_limits_count_unicode_characters() {
        assert_eq!(safe_diagnostic("ééé", 2), "éé");
        assert_eq!(
            safe_diagnostic(" a\0\u{85}\u{200b}\u{202e}\u{a0}\u{2029}b\t c ", 50),
            "a b c"
        );
        assert_eq!(
            safe_diagnostic(&format!("{}tail", "é".repeat(500)), 500),
            "é".repeat(500)
        );
        let error = classify_rpc_error(&json!({"code": -32005, "message": "é".repeat(600), "data": {"policy": "é".repeat(250)}})).expect("metadata recognized");
        assert_eq!(
            error.policy.expect("policy retained").chars().count(),
            py_rpc::POLICY_METADATA_LIMIT
        );
        assert_eq!(
            error
                .message
                .strip_prefix("RPC endpoint refused the request under its traffic policy: ")
                .expect("wording retained")
                .chars()
                .count(),
            py_rpc::POLICY_DETAIL_LIMIT
        );
    }

    #[test]
    fn response_detail_matches_python_containers_and_body_bound() {
        for (body, expected) in [
            ("null", "None"),
            ("true", "True"),
            ("[1,\"x\",null,false]", "[1, 'x', None, False]"),
            ("{\"error\":false,\"message\":\"fallback\"}", "fallback"),
            ("{\"error\":{\"message\":\"busy\"}}", "{'message': 'busy'}"),
            ("{\"z\":1,\"a\":2}", "{'z': 1, 'a': 2}"),
            ("\u{1c}{\"error\":\"busy\"}\u{1f}", "busy"),
            ("[\"\u{10d40}\"]", "['\\U00010d40']"),
        ] {
            assert_eq!(response_detail(body.as_bytes()), expected);
        }
        assert_eq!(response_detail(&[b'a', 0xff, b'b']), "a�b");
        assert_eq!(
            response_detail(&vec![b'a'; 5000]).len(),
            py_rpc::POLICY_DETAIL_LIMIT
        );
        let mut body = vec![b' '; py_rpc::POLICY_BODY_LIMIT];
        body.extend_from_slice(b"hidden");
        assert_eq!(response_detail(&body), "");
    }

    #[test]
    fn retry_duration_preserves_raw_seconds_milliseconds_and_http_date() {
        let seconds = classify_rpc_error(&json!({"code": -32005, "data": {"retry_after": 1.5}}))
            .expect("metadata recognized");
        assert_eq!(seconds.retry_after.as_deref(), Some("1.5"));
        assert_eq!(
            seconds.retry_after_duration(),
            Some(Duration::from_millis(1500))
        );
        for key in ["retry_after_ms", "retryAfterMs"] {
            let mut data = serde_json::Map::new();
            data.insert(key.to_owned(), json!(1500));
            let milliseconds = classify_rpc_error(&json!({"code": -32005, "data": data}))
                .expect("metadata recognized");
            assert_eq!(milliseconds.retry_after, None);
            assert_eq!(
                milliseconds.retry_after_duration(),
                Some(Duration::from_millis(1500))
            );
        }
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        let deadline = httpdate::parse_http_date(date).expect("valid fixture date");
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", http::HeaderValue::from_static(date));
        let refusal = classify_handshake(429, &headers, None).expect("policy status");
        assert_eq!(
            refusal.retry_after_duration_at(deadline - Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            refusal.retry_after_duration_at(deadline + Duration::from_secs(30)),
            Some(Duration::ZERO)
        );
        for value in ["-1", "NaN", "inf", "unknown", "1e999"] {
            let refusal =
                classify_rpc_error(&json!({"code": -32005, "data": {"retryAfter": value}}))
                    .expect("metadata recognized");
            assert_eq!(refusal.retry_after_duration(), None);
        }
        let max = classify_rpc_error(
            &json!({"code": -32005, "data": {"retryAfter": u64::MAX.to_string()}}),
        )
        .expect("metadata recognized");
        assert_eq!(
            max.retry_after_duration(),
            Some(Duration::from_secs(u64::MAX))
        );
    }

    #[test]
    fn handshake_first_header_reset_fallback_and_exact_wording() {
        let mut headers = HeaderMap::new();
        headers.append(
            "x-ratelimit-policy",
            http::HeaderValue::from_static("first"),
        );
        headers.append(
            "x-ratelimit-policy",
            http::HeaderValue::from_static("second"),
        );
        headers.insert(
            "x-ratelimit-reason",
            http::HeaderValue::from_static("source"),
        );
        headers.insert("retry-after", http::HeaderValue::from_static(" \t "));
        headers.insert("x-ratelimit-reset", http::HeaderValue::from_static("60"));
        let refusal = classify_handshake(429, &headers, Some(b"{\"error\":\"Busy.\"}"))
            .expect("policy status");
        assert_eq!(
            refusal.message,
            "RPC endpoint rate limited this source (HTTP 429): Busy. (policy=first, reason=source)"
        );
        assert_eq!(refusal.retry_after.as_deref(), Some("60"));
        assert_eq!(refusal.status_code, Some(429));
        assert_eq!(refusal.kind, RefusalKind::Handshake { status: 429 });
        assert_eq!(
            classify_handshake(403, &HeaderMap::new(), None)
                .expect("policy status")
                .message,
            "RPC endpoint rejected this source under its access policy (HTTP 403)"
        );
        assert!(classify_handshake(502, &headers, Some(b"busy")).is_none());
    }

    // pyt: sdk/python/tests/unit/test_rpc.py::test_handshake_diagnostics_strip_terminal_controls
    #[test]
    fn handshake_diagnostics_strip_terminal_controls_in_utf8_headers_and_body() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-policy",
            http::HeaderValue::from_bytes("public-\u{202e}rpc".as_bytes())
                .expect("UTF-8 obs-text is representable in HeaderMap"),
        );
        let refusal = classify_handshake(
            429,
            &headers,
            Some(b"{\"error\":\"slow down\\u001b]8;;https://evil.example\\u0007\\u202eclick\"}"),
        )
        .expect("policy status");
        assert_eq!(refusal.policy.as_deref(), Some("public- rpc"));
        for forbidden in ['\u{1b}', '\u{7}', '\u{202e}'] {
            assert!(!refusal.message.contains(forbidden));
            assert!(
                !refusal
                    .policy
                    .as_deref()
                    .unwrap_or_default()
                    .contains(forbidden)
            );
        }
    }

    #[test]
    fn close_policy_requires_exact_code_and_retains_sanitized_reason() {
        let refusal = classify_close(1008, "slow\u{1b}[2J\u{202e}down").expect("policy close");
        assert_eq!(refusal.kind, RefusalKind::Close1008);
        assert_eq!(refusal.status_code, Some(1008));
        assert_eq!(refusal.reason.as_deref(), Some("slow [2J down"));
        assert_eq!(
            refusal.message,
            "RPC endpoint closed the connection for a policy violation (WebSocket 1008): slow [2J down"
        );
        assert_eq!(
            classify_close(1008, "\0\t").expect("policy close").reason,
            None
        );
        assert!(classify_close(1000, "policy").is_none());
        assert!(classify_close(1011, "policy").is_none());
    }

    #[test]
    fn safe_endpoint_redacts_secrets_and_preserves_explicit_ports() {
        for (raw, expected) in [
            (
                "wss://user:password@rpc.example/private?api_key=secret#fragment",
                "wss://rpc.example",
            ),
            ("wss://rpc.example:443/token", "wss://rpc.example:443"),
            ("ws://[2001:db8::1]:9944/token", "ws://[2001:db8::1]:9944"),
            ("wss://rpc.example:000443/token", "wss://rpc.example:443"),
            (
                "wss://rpc.example:65536/token",
                "the configured RPC endpoint",
            ),
            ("not-an-endpoint", "the configured RPC endpoint"),
            ("/relative", "the configured RPC endpoint"),
        ] {
            assert_eq!(safe_endpoint(raw), expected);
        }
    }

    #[test]
    fn python_error_formatting_preserves_pinned_payload_rules() {
        for (error, expected) in [
            (json!({"message": "boom"}), "boom"),
            (json!({"message": "boom", "data": "detail"}), "boom: detail"),
            (json!({"message": "boom", "data": false}), "boom: False"),
            (
                json!({"message": "boom", "data": {"value": null}}),
                "boom: {'value': None}",
            ),
            (json!({"message": "", "data": []}), "Request failed: []"),
        ] {
            assert_eq!(format_rpc_error(&error), expected);
        }
        assert_eq!(python_string_repr("can't"), "\"can't\"");
        assert_eq!(python_string_repr("a\n\u{a0}b"), "'a\\n\\xa0b'");
        assert_eq!(python_string_repr("one's \"two\""), "'one\\'s \"two\"'");
        for (raw, expected) in [
            ("1e-5", "1e-05"),
            ("1e-4", "0.0001"),
            ("1e15", "1000000000000000.0"),
            ("1e16", "1e+16"),
            ("-0.0", "-0.0"),
            ("-0", "0"),
            (
                "123456789012345678901234567890",
                "123456789012345678901234567890",
            ),
        ] {
            let parsed: Value = serde_json::from_str(raw).expect("valid fixture number");
            assert_eq!(python_string(&parsed), expected);
        }
    }
}
