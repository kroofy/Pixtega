//! Shared domain types used across request parsing, source adapters, image
//! processing, and the HTTP application.
//!
//! Project terminology: Source, Transport, Mount, Key Prefix, Source
//! Object, Transform, Derived Image, Width Allowlist.

use std::fmt;

/// Output encoding formats the service can produce.
///
/// A closed set. Aliases such as `jpg` are intentionally not
/// representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutputFormat {
    Webp,
    Avif,
    Jpeg,
}

impl OutputFormat {
    /// Canonical lowercase name as it appears in the transform segment and
    /// configuration (`webp`, `avif`, `jpeg`).
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Webp => "webp",
            OutputFormat::Avif => "avif",
            OutputFormat::Jpeg => "jpeg",
        }
    }

    /// Response `Content-Type` for this format.
    pub fn content_type(self) -> &'static str {
        match self {
            OutputFormat::Webp => "image/webp",
            OutputFormat::Avif => "image/avif",
            OutputFormat::Jpeg => "image/jpeg",
        }
    }

    /// Parse a canonical lowercase format name. Aliases are rejected.
    pub fn from_canonical(s: &str) -> Option<Self> {
        match s {
            "webp" => Some(OutputFormat::Webp),
            "avif" => Some(OutputFormat::Avif),
            "jpeg" => Some(OutputFormat::Jpeg),
            _ => None,
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A fully resolved Transform: requested output width, format, and the
/// resolved encoder quality.
///
/// `quality` is always resolved by the request parser from the selected
/// format policy: either the explicitly requested allowed quality or the
/// policy default when `q` was omitted. An explicit quality equal to the
/// default is rejected during parsing, so a `Transform` never records
/// whether the default was spelled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    pub width: u32,
    pub format: OutputFormat,
    pub quality: u32,
}

/// The upstream key for a Source Object: the configured Key Prefix segments
/// followed by the validated, exactly-once-decoded source path segments.
///
/// Segments are stored decoded (UTF-8). Adapters that need to place them on
/// a wire (HTTP URL, S3 key) re-encode them with their own rules; the
/// filesystem adapter uses them as path components directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamKey {
    pub segments: Vec<String>,
}

impl UpstreamKey {
    pub fn new(segments: Vec<String>) -> Self {
        UpstreamKey { segments }
    }

    /// The `/`-joined representation (S3 object key).
    pub fn joined(&self) -> String {
        self.segments.join("/")
    }
}

/// The result of parsing and validating one HTTP request, before any Source
/// I/O happens. The rest of the application never re-parses URL strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    /// The Mount that selected the Source.
    pub mount: String,
    /// Key Prefix + validated source path, ready for the Source adapter.
    pub upstream_key: UpstreamKey,
    /// The resolved Transform.
    pub transform: Transform,
    /// Whether the request counts as versioned: it carried a valid
    /// non-empty `v` query parameter and the configured `version_token`
    /// mode is `accept`. Only the cache policy depends on this; the
    /// derived bytes do not.
    pub versioned: bool,
}

/// An upstream object validator, when the Transport exposed one.
///
/// `validator` is the opaque tag without quotes or a `W/` prefix.
/// Filesystem identity is weak. Missing or pre-epoch mtime is no identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectIdentity {
    pub validator: String,
    pub weak: bool,
}

impl ObjectIdentity {
    pub fn strong(validator: impl Into<String>) -> Self {
        ObjectIdentity {
            validator: validator.into(),
            weak: false,
        }
    }

    pub fn weak(validator: impl Into<String>) -> Self {
        ObjectIdentity {
            validator: validator.into(),
            weak: true,
        }
    }
}

/// Identity resolved without reading object bytes (`Source::identify`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedObject {
    pub identity: ObjectIdentity,
    pub upstream_status: Option<u16>,
}

/// Bytes successfully read from a Source, plus the upstream protocol status
/// when the Transport has one (HTTP status for HTTP(S) and S3 adapters).
#[derive(Debug, Clone)]
pub struct FetchedObject {
    pub bytes: Vec<u8>,
    pub upstream_status: Option<u16>,
    /// Present when the Transport exposed an object validator (S3 ETag,
    /// HTTP `ETag`, or a usable filesystem mtime+size+key).
    pub identity: Option<ObjectIdentity>,
}
