//! Request parsing and policy.
//!
//! Turns one raw HTTP request target into a [`ResolvedRequest`] or a typed
//! [`RequestError`], before any Source is contacted. This module owns:
//!
//! - the 8192-byte request-target limit
//! - the configured path prefix
//! - Mount selection
//! - source-path validation (exactly-once decoding, traversal rejection,
//!   canonical percent encoding, UTF-8)
//! - the Transform grammar and its allowlists
//! - the `v` query parameter contract
//!
//! The rest of the application never parses URL strings again.
//!
//! Error precedence: target length, then path prefix, then Mount, then
//! path structure (missing source path / transform), then source-path
//! segments left to right, then the Transform segment, then the query.

use crate::config::{AppConfig, VersionTokenMode};
use crate::errors::RequestError;
use crate::types::{OutputFormat, ResolvedRequest, Transform, UpstreamKey};

/// Maximum accepted request-target length in bytes (path + `?` + query).
pub const MAX_TARGET_BYTES: usize = 8192;

/// Configuration caps every allowed width at 16384, so a syntactically
/// canonical width above this can never be in a Width Allowlist. Such
/// values (including ones that overflow integer parsing) are reported as
/// disallowed widths rather than grammar errors.
const MAX_CONFIGURABLE_WIDTH: u64 = 16384;

/// Configuration caps qualities at 100; larger canonical decimals can never
/// be allowed for any format.
const MAX_CONFIGURABLE_QUALITY: u64 = 100;

/// Parse and validate one GET request target.
///
/// `raw_path` is the raw, still-percent-encoded URI path. `raw_query` is
/// the raw query string without the leading `?`, when one exists.
///
/// On success the returned [`ResolvedRequest`] carries the Mount, the
/// upstream key (Key Prefix applied), the resolved Transform, and whether
/// the request was versioned. The Source is guaranteed to be configured.
///
/// This function is pure: it performs no Source I/O of any kind.
pub fn parse_request(
    config: &AppConfig,
    raw_path: &str,
    raw_query: Option<&str>,
) -> Result<ResolvedRequest, RequestError> {
    let target_len = raw_path.len() + raw_query.map_or(0, |q| 1 + q.len());
    if target_len > MAX_TARGET_BYTES {
        return Err(RequestError::TargetTooLong);
    }

    let rest = raw_path
        .strip_prefix(config.path_prefix.as_str())
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(RequestError::InvalidPrefix)?;

    // Layout after the prefix: {mount}/{source-path...}/{transform}.
    let segments: Vec<&str> = rest.split('/').collect();
    let mount = segments[0];
    if mount.is_empty() {
        return Err(RequestError::MissingMount);
    }
    // Mounts are matched literally; a percent-encoded or otherwise malformed
    // mount simply matches nothing.
    let source = config
        .source_for_mount(mount)
        .ok_or(RequestError::UnknownMount)?;

    match segments.len() {
        1 => return Err(RequestError::MissingSourcePath),
        2 => return Err(RequestError::MissingTransform),
        _ => {}
    }
    let source_segments = &segments[1..segments.len() - 1];
    let transform_segment = segments[segments.len() - 1];

    let mut key_segments = source.key_prefix_segments().to_vec();
    for raw_segment in source_segments {
        key_segments.push(validate_and_decode_segment(raw_segment)?);
    }

    let transform = parse_transform(config, transform_segment)?;
    let versioned = parse_query(raw_query, config.version_token)?;

    Ok(ResolvedRequest {
        mount: mount.to_string(),
        upstream_key: UpstreamKey::new(key_segments),
        transform,
        versioned,
    })
}

/// RFC 3986 unreserved ASCII: `A-Z a-z 0-9 - . _ ~`.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Hex digit value accepting canonical (uppercase) hex only. Lowercase hex
/// in a percent triplet is a second spelling of the same byte and is
/// rejected.
fn canonical_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Case-insensitive hex digit value, used only by the second-decoding
/// traversal check, which must catch attacks in either case.
fn any_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Validate one raw source-path segment and decode it exactly once.
///
/// On the wire a segment may contain only unreserved ASCII characters and
/// percent triplets with uppercase hex. Rejects: empty segments, invalid or
/// non-canonical percent encoding (including encoded unreserved ASCII),
/// encoded `/` or `\`, NUL and ASCII control bytes (raw or decoded), any
/// other literal byte outside the allowed set (backslash, space, `&`, ...),
/// non-UTF-8 decoded bytes, `.`/`..` segments, and double-encoded traversal
/// or path-delimiter sequences.
fn validate_and_decode_segment(raw: &str) -> Result<String, RequestError> {
    if raw.is_empty() {
        return Err(RequestError::InvalidSourcePath);
    }
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'%' {
            if i + 2 >= bytes.len() {
                return Err(RequestError::InvalidSourcePath);
            }
            let hi = canonical_hex_value(bytes[i + 1]).ok_or(RequestError::InvalidSourcePath)?;
            let lo = canonical_hex_value(bytes[i + 2]).ok_or(RequestError::InvalidSourcePath)?;
            let decoded = hi * 16 + lo;
            if is_unreserved(decoded) {
                // Encoding an unreserved character creates a second spelling
                // of the same path; this also covers %2E-style dot segments.
                return Err(RequestError::InvalidSourcePath);
            }
            if decoded == b'/' || decoded == b'\\' {
                return Err(RequestError::InvalidSourcePath);
            }
            if decoded < 0x20 || decoded == 0x7F {
                return Err(RequestError::InvalidSourcePath);
            }
            out.push(decoded);
            i += 3;
        } else if is_unreserved(byte) {
            out.push(byte);
            i += 1;
        } else {
            // Literal byte outside the wire alphabet: backslash, control
            // characters, space, '&', ':', non-ASCII, ...
            return Err(RequestError::InvalidSourcePath);
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| RequestError::InvalidSourcePath)?;
    if decoded == "." || decoded == ".." {
        return Err(RequestError::InvalidSourcePath);
    }
    reject_double_encoded_traversal(&decoded)?;
    Ok(decoded)
}

/// Reject a segment whose second decoding would turn the complete segment
/// into `.` or `..`, or would introduce `/` or `\`.
///
/// The second decode is deliberately lenient (case-insensitive hex, invalid
/// triplets pass through) because it models what a careless downstream
/// decoder might do; it never changes the accepted path.
fn reject_double_encoded_traversal(decoded: &str) -> Result<(), RequestError> {
    let bytes = decoded.as_bytes();
    let mut twice: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (any_hex_value(bytes[i + 1]), any_hex_value(bytes[i + 2]))
            {
                twice.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        twice.push(bytes[i]);
        i += 1;
    }
    if twice == b"." || twice == b".." || twice.contains(&b'/') || twice.contains(&b'\\') {
        return Err(RequestError::InvalidSourcePath);
    }
    Ok(())
}

/// A canonical decimal: ASCII digits only, no sign, no leading zero unless
/// the entire value is `0`.
fn is_canonical_decimal(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

/// Parse and police the final transform segment:
/// `w<canonical-decimal>[,q<canonical-decimal>].<format>`.
///
/// The grammar admits no `%`, so any percent-encoded transform fails as
/// [`RequestError::InvalidTransform`].
fn parse_transform(config: &AppConfig, raw: &str) -> Result<Transform, RequestError> {
    // Exactly one dot separates the fields from the format.
    let (fields_part, format_part) = match raw.split_once('.') {
        Some((fields, format)) if !format.contains('.') => (fields, format),
        _ => return Err(RequestError::InvalidTransform),
    };
    let format = OutputFormat::from_canonical(format_part).ok_or(RequestError::InvalidTransform)?;

    let mut fields = fields_part.split(',');
    let width_field = fields.next().unwrap_or("");
    let quality_field = fields.next();
    if fields.next().is_some() {
        return Err(RequestError::InvalidTransform);
    }

    let width_digits = width_field
        .strip_prefix('w')
        .ok_or(RequestError::InvalidTransform)?;
    if !is_canonical_decimal(width_digits) {
        return Err(RequestError::InvalidTransform);
    }
    let quality_digits = match quality_field {
        None => None,
        Some(field) => {
            let digits = field
                .strip_prefix('q')
                .ok_or(RequestError::InvalidTransform)?;
            if !is_canonical_decimal(digits) {
                return Err(RequestError::InvalidTransform);
            }
            Some(digits)
        }
    };

    // Policy checks: width allowlist, then format policy, then quality.
    let width_value: u64 = width_digits
        .parse()
        .map_err(|_| RequestError::DisallowedWidth)?;
    if width_value > MAX_CONFIGURABLE_WIDTH {
        return Err(RequestError::DisallowedWidth);
    }
    let width = width_value as u32;
    if !config.allowed_widths.contains(&width) {
        return Err(RequestError::DisallowedWidth);
    }

    let policy = config
        .format_policy(format)
        .ok_or(RequestError::UnconfiguredFormat)?;

    let quality = match quality_digits {
        None => policy.default_quality,
        Some(digits) => {
            let value: u64 = digits
                .parse()
                .map_err(|_| RequestError::DisallowedQuality)?;
            if value > MAX_CONFIGURABLE_QUALITY {
                return Err(RequestError::DisallowedQuality);
            }
            let quality = value as u32;
            if quality == policy.default_quality {
                return Err(RequestError::QualityEqualsDefault);
            }
            if !policy.allowed_qualities.contains(&quality) {
                return Err(RequestError::DisallowedQuality);
            }
            quality
        }
    };

    Ok(Transform {
        width,
        format,
        quality,
    })
}

/// Validate the query string. Only `v` is known; it may appear at most
/// once. Returns whether the request counts as versioned. An empty query
/// string is treated as no query.
///
/// `mode` decides what a well-formed `v` means: `accept` marks the request
/// versioned, `ignore` validates the same grammar but reports the request
/// as unversioned, and `reject` treats `v` like any other unknown query
/// parameter.
fn parse_query(raw_query: Option<&str>, mode: VersionTokenMode) -> Result<bool, RequestError> {
    let query = match raw_query {
        None => return Ok(false),
        Some("") => return Ok(false),
        Some(q) => q,
    };
    let mut versioned = false;
    for pair in query.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (pair, None),
        };
        if key != "v" || mode == VersionTokenMode::Reject {
            return Err(RequestError::InvalidQuery);
        }
        if versioned {
            return Err(RequestError::InvalidQuery);
        }
        // `?v` (present but valueless) is a present-but-invalid version.
        let value = value.ok_or(RequestError::InvalidVersion)?;
        validate_version_token(value)?;
        versioned = true;
    }
    Ok(versioned && mode == VersionTokenMode::Accept)
}

/// `v` must match `[A-Za-z0-9._~-]{1,128}` exactly; percent encoding is not
/// accepted.
fn validate_version_token(value: &str) -> Result<(), RequestError> {
    if value.is_empty() || value.len() > 128 {
        return Err(RequestError::InvalidVersion);
    }
    if !value.bytes().all(is_unreserved) {
        return Err(RequestError::InvalidVersion);
    }
    Ok(())
}
