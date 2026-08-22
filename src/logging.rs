//! Structured JSON logs on standard output.
//!
//! One completion event per request. Fields are stable and low-cardinality;
//! response bodies, credentials, and the source-version query value are
//! never logged.

use serde::Serialize;

use crate::errors::Outcome;
use crate::types::OutputFormat;

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

    pub fn with_transform(mut self, width: u32, format: OutputFormat) -> Self {
        self.width = Some(width);
        self.format = Some(format.as_str());
        self
    }

    /// Write the event as one JSON line to standard output.
    pub fn emit(&self) {
        // serde_json can only fail here on non-string map keys; this struct
        // has none, so a failure is unreachable.
        if let Ok(line) = serde_json::to_string(self) {
            println!("{line}");
        }
    }
}
