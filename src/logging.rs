//! Structured JSON logs on standard output.
//!
//! One completion event per request. Fields are stable and low-cardinality;
//! response bodies, credentials, and the source-version query value are
//! never logged. Source failures with an internal `detail` emit a separate
//! warn event immediately before the completion line.

use serde::Serialize;

use crate::errors::Outcome;

/// One request-completion event, serialized as a single JSON line.
#[derive(Debug, Serialize)]
pub struct CompletionEvent {
    pub event: &'static str,
    pub status: u16,
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    pub elapsed_ms: u64,
}

impl CompletionEvent {
    pub fn new(status: u16, outcome: Outcome, elapsed_ms: u64) -> Self {
        CompletionEvent {
            event: "request_completed",
            status,
            outcome: outcome.as_str(),
            mount: None,
            width: None,
            format: None,
            upstream_status: None,
            input_bytes: None,
            output_bytes: None,
            elapsed_ms,
        }
    }

    /// Write the event as one JSON line to standard output.
    pub fn emit(&self) {
        emit_json(self);
    }
}

/// Warn-level diagnostic for a source failure. Carries the adapter `detail`
/// string (already log-safe: no URLs, credentials, or response bodies).
/// Kept off [`CompletionEvent`] so that event's field set stays closed
/// and low-cardinality.
#[derive(Debug, Serialize)]
pub struct SourceErrorEvent {
    pub event: &'static str,
    pub level: &'static str,
    pub detail: String,
}

impl SourceErrorEvent {
    pub fn new(detail: impl Into<String>) -> Self {
        SourceErrorEvent {
            event: "source_error",
            level: "warn",
            detail: detail.into(),
        }
    }

    pub fn emit(&self) {
        emit_json(self);
    }
}

fn emit_json<T: Serialize>(value: &T) {
    // serde_json can only fail here on non-string map keys; these structs
    // have none, so a failure is unreachable.
    if let Ok(line) = serde_json::to_string(value) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::Outcome;

    #[test]
    fn source_error_event_carries_detail_at_warn() {
        let event = SourceErrorEvent::new("s3 dispatch failure");
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "source_error");
        assert_eq!(json["level"], "warn");
        assert_eq!(json["detail"], "s3 dispatch failure");
    }

    #[test]
    fn completion_event_does_not_grow_a_detail_field() {
        let event = CompletionEvent::new(502, Outcome::SourceUnavailable, 12);
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("detail").is_none());
        assert_eq!(json["event"], "request_completed");
        assert_eq!(json["outcome"], "source_unavailable");
    }
}
