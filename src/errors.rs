//! Error taxonomy.
//!
//! Three separate error classes:
//!
//! - [`RequestError`]: the request itself is invalid (all map to 400).
//! - [`SourceError`]: fetching the Source Object failed (404/502/504).
//! - [`ProcessError`]: image processing failed (502 for bad source content,
//!   500 for failures in the service's own pipeline).
//!
//! Variants encode the HTTP status taxonomy and the closed observability
//! outcome set; nothing matches on message strings.

use std::fmt;

/// Stable, low-cardinality request outcome for the completion log event.
/// This is a closed set; do not add variants without updating the
/// documented outcome set (README "Observability").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    RejectedRequest,
    NotFound,
    Timeout,
    SourceTooLarge,
    SourceUnavailable,
    UndecodableSource,
    ResizeFailed,
    FlattenFailed,
    EncodeFailed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::RejectedRequest => "rejected_request",
            Outcome::NotFound => "not_found",
            Outcome::Timeout => "timeout",
            Outcome::SourceTooLarge => "source_too_large",
            Outcome::SourceUnavailable => "source_unavailable",
            Outcome::UndecodableSource => "undecodable_source",
            Outcome::ResizeFailed => "resize_failed",
            Outcome::FlattenFailed => "flatten_failed",
            Outcome::EncodeFailed => "encode_failed",
        }
    }
}

/// A request that fails validation before any Source is contacted.
/// Every variant maps to HTTP 400 (405 is handled by the HTTP layer for
/// non-GET methods, before parsing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// Request target (path + query) longer than 8192 bytes.
    TargetTooLong,
    /// Path does not start with the configured path prefix.
    InvalidPrefix,
    /// No Mount segment present.
    MissingMount,
    /// Mount segment present but not configured.
    UnknownMount,
    /// No source path between the Mount and the Transform segment.
    MissingSourcePath,
    /// Source path failed validation (traversal, encoding, control bytes...).
    InvalidSourcePath,
    /// No transform segment present.
    MissingTransform,
    /// Transform segment failed the grammar (fields, order, canonical
    /// decimals, format aliases, extra dots...).
    InvalidTransform,
    /// Width parsed but is not in the Width Allowlist.
    DisallowedWidth,
    /// Format parsed but has no configured format policy.
    UnconfiguredFormat,
    /// Quality parsed but is not in the selected format policy's allowlist.
    DisallowedQuality,
    /// Explicit quality equal to the selected format's default; omitting `q`
    /// is the canonical spelling.
    QualityEqualsDefault,
    /// Malformed query string, unknown parameter, or repeated parameter.
    InvalidQuery,
    /// `v` present but empty, overlong, percent-encoded, or non-canonical.
    InvalidVersion,
}

impl RequestError {
    pub fn status(&self) -> u16 {
        400
    }

    pub fn outcome(&self) -> Outcome {
        Outcome::RejectedRequest
    }

    /// Stable public message. Never contains caller input.
    pub fn public_message(&self) -> &'static str {
        match self {
            RequestError::TargetTooLong => "request target too long",
            RequestError::InvalidPrefix => "unknown path prefix",
            RequestError::MissingMount => "missing mount",
            RequestError::UnknownMount => "unknown mount",
            RequestError::MissingSourcePath => "missing source path",
            RequestError::InvalidSourcePath => "invalid source path",
            RequestError::MissingTransform => "missing transform",
            RequestError::InvalidTransform => "invalid transform",
            RequestError::DisallowedWidth => "width not allowed",
            RequestError::UnconfiguredFormat => "format not configured",
            RequestError::DisallowedQuality => "quality not allowed",
            RequestError::QualityEqualsDefault => "quality equals the format default; omit q",
            RequestError::InvalidQuery => "invalid query parameter",
            RequestError::InvalidVersion => "invalid version token",
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.public_message())
    }
}

impl std::error::Error for RequestError {}

/// A failure to obtain the Source Object from a Source.
///
/// `detail` fields are internal diagnostics for logs only. They must never
/// reach a client-facing response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The Source answered authoritatively that the object is absent
    /// (HTTP 404/410, S3 NoSuchKey/NotFound, missing regular file). 404.
    NotFound { upstream_status: Option<u16> },
    /// The Source answered but did not provide a usable object: permission
    /// denial, unexpected upstream status, content-encoding, non-regular
    /// file, transport error... 502.
    Unavailable {
        upstream_status: Option<u16>,
        detail: String,
    },
    /// The object is larger than `max_download_bytes` (advertised or
    /// observed while streaming). 502.
    TooLarge { upstream_status: Option<u16> },
    /// Fetching the Source timed out. 504.
    Timeout,
}

impl SourceError {
    pub fn status(&self) -> u16 {
        match self {
            SourceError::NotFound { .. } => 404,
            SourceError::Unavailable { .. } => 502,
            SourceError::TooLarge { .. } => 502,
            SourceError::Timeout => 504,
        }
    }

    pub fn outcome(&self) -> Outcome {
        match self {
            SourceError::NotFound { .. } => Outcome::NotFound,
            SourceError::Unavailable { .. } => Outcome::SourceUnavailable,
            SourceError::TooLarge { .. } => Outcome::SourceTooLarge,
            SourceError::Timeout => Outcome::Timeout,
        }
    }

    pub fn upstream_status(&self) -> Option<u16> {
        match self {
            SourceError::NotFound { upstream_status } => *upstream_status,
            SourceError::Unavailable {
                upstream_status, ..
            } => *upstream_status,
            SourceError::TooLarge { upstream_status } => *upstream_status,
            SourceError::Timeout => None,
        }
    }

    /// Stable public message. Never contains upstream response bodies,
    /// credentials, or URLs.
    pub fn public_message(&self) -> &'static str {
        match self {
            SourceError::NotFound { .. } => "source object not found",
            SourceError::Unavailable { .. } => "source unavailable",
            SourceError::TooLarge { .. } => "source object too large",
            SourceError::Timeout => "source fetch timed out",
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.public_message())
    }
}

impl std::error::Error for SourceError {}

/// A failure while turning fetched source bytes into a Derived Image.
///
/// Source-content problems (bytes we refuse to process) are 502; failures of
/// the service's own pipeline after a valid source image was accepted are 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// Bytes are not a decodable JPEG/PNG/WebP/AVIF raster image, or are an
    /// animated/multi-page/document input we refuse. 502.
    Undecodable { detail: String },
    /// Oriented width * height exceeds the configured megapixel limit, or
    /// the dimension arithmetic would overflow. 502.
    TooManyPixels,
    /// Resize failed after a valid source image was accepted. 500.
    Resize { detail: String },
    /// Alpha flattening failed. 500.
    Flatten { detail: String },
    /// Encoding failed. 500.
    Encode { detail: String },
}

impl ProcessError {
    pub fn status(&self) -> u16 {
        match self {
            ProcessError::Undecodable { .. } => 502,
            ProcessError::TooManyPixels => 502,
            ProcessError::Resize { .. } => 500,
            ProcessError::Flatten { .. } => 500,
            ProcessError::Encode { .. } => 500,
        }
    }

    pub fn outcome(&self) -> Outcome {
        match self {
            ProcessError::Undecodable { .. } => Outcome::UndecodableSource,
            ProcessError::TooManyPixels => Outcome::SourceTooLarge,
            ProcessError::Resize { .. } => Outcome::ResizeFailed,
            ProcessError::Flatten { .. } => Outcome::FlattenFailed,
            ProcessError::Encode { .. } => Outcome::EncodeFailed,
        }
    }

    /// Stable public message. Never contains internal diagnostics.
    pub fn public_message(&self) -> &'static str {
        match self {
            ProcessError::Undecodable { .. } => "source is not a supported image",
            ProcessError::TooManyPixels => "source image exceeds pixel limit",
            ProcessError::Resize { .. } => "image resize failed",
            ProcessError::Flatten { .. } => "image flatten failed",
            ProcessError::Encode { .. } => "image encode failed",
        }
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.public_message())
    }
}

impl std::error::Error for ProcessError {}

/// A configuration problem that must stop startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub message: String,
}

impl ConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        ConfigError {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "configuration error: {}", self.message)
    }
}

impl std::error::Error for ConfigError {}
