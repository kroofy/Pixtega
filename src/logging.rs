//! Structured JSON logs on standard output.
//!
//! One completion event per request. Fields are stable and low-cardinality;
//! response bodies and credentials are never logged. Source failures with an internal `detail` emit a separate
//! warn event immediately before the completion line.

use serde::{Serialize, Serializer};

use crate::errors::Outcome;
use crate::types::OutputFormat;

/// What one request established about itself, gathered while answering it
/// and written as its single completion event. Only fields that were
/// actually established are set; the event omits the rest.
#[derive(Debug, Serialize)]
pub struct Completion<'a> {
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
}

impl<'a> Completion<'a> {
    pub fn new(outcome: Outcome) -> Self {
        Completion {
            outcome,
            mount: None,
            width: None,
            format: None,
            upstream_status: None,
            input_bytes: None,
            output_bytes: None,
        }
    }

    /// Write the completion event as one JSON line to standard output, with
    /// the response status and the time the whole request took.
    pub fn emit(&self, status: u16, elapsed_ms: u64) {
        emit_json(&self.event(status, elapsed_ms));
    }

    fn event(&self, status: u16, elapsed_ms: u64) -> CompletionEvent<'_> {
        CompletionEvent {
            event: "request_completed",
            status,
            completion: self,
            elapsed_ms,
        }
    }
}

/// The completion event as it is written: the request's [`Completion`]
/// between the status and the elapsed time.
#[derive(Serialize)]
struct CompletionEvent<'a> {
    event: &'static str,
    status: u16,
    #[serde(flatten)]
    completion: &'a Completion<'a>,
    elapsed_ms: u64,
}

// The log spells an outcome and a format exactly as `as_str` does.
impl Serialize for Outcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for OutputFormat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Warn-level diagnostic for a source failure. Carries the adapter `detail`
/// string (already log-safe: no URLs, credentials, or response bodies).
/// Kept off [`Completion`] so that event's field set stays closed
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
    fn completion_event_does_not_grow_a_detail_field() {
        let completion = Completion::new(Outcome::SourceUnavailable);
        let json = serde_json::to_value(completion.event(502, 12)).unwrap();
        assert!(json.get("detail").is_none());
        assert_eq!(json["event"], "request_completed");
        assert_eq!(json["outcome"], "source_unavailable");
    }
}
