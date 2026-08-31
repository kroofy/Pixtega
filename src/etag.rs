//! Derived-image validators.
//!
//! A derived ETag is the upstream object identity plus the resolved
//! Transform plus a compile-time generation. Weak upstream validators stay
//! weak on the way out. There is no `Last-Modified`.

use crate::types::{ObjectIdentity, Transform};

/// Bump when encoder output can change for the same source bytes and
/// Transform, so a revalidation cannot 304 yesterday's pixels.
pub const DERIVATION_GENERATION: u32 = 1;

/// An ETag value ready for the `ETag` response header (`W/"…"` or `"…"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedETag(String);

impl DerivedETag {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// RFC 9110 weak comparison: the opaque tags match, `W/` is ignored.
    pub fn matches_if_none_match(&self, header: &str) -> bool {
        let Some(want) = opaque_tag(&self.0) else {
            return false;
        };
        parse_if_none_match(header).iter().any(|got| got == &want)
    }
}

/// Build the derived validator for one identity + Transform.
pub fn derived_etag(identity: &ObjectIdentity, transform: &Transform) -> DerivedETag {
    let opaque = format!(
        "{gen}:{width}:{format}:{quality}:{validator}",
        gen = DERIVATION_GENERATION,
        width = transform.width,
        format = transform.format.as_str(),
        quality = transform.quality,
        validator = percent_encode(&identity.validator),
    );
    let quoted = format!("\"{opaque}\"");
    DerivedETag(if identity.weak {
        format!("W/{quoted}")
    } else {
        quoted
    })
}

/// Parse one upstream `ETag` header into an identity. Empty or unparseable
/// values are absence of identity, not an error.
pub fn parse_entity_tag(raw: &str) -> Option<ObjectIdentity> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (weak, rest) = match raw.strip_prefix("W/") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, raw),
    };
    let validator = unquote_or_token(rest)?;
    if validator.is_empty() {
        return None;
    }
    Some(ObjectIdentity { validator, weak })
}

fn unquote_or_token(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('"')?;
    let end = inner.find('"')?;
    if end + 1 != inner.len() {
        return None;
    }
    Some(inner[..end].to_string())
}

fn opaque_tag(etag: &str) -> Option<String> {
    parse_entity_tag(etag).map(|identity| identity.validator)
}

/// Opaque tags from `If-None-Match`. `*` is ignored (no representation
/// exists yet that we would claim).
fn parse_if_none_match(header: &str) -> Vec<String> {
    let header = header.trim();
    if header.is_empty() || header == "*" {
        return Vec::new();
    }
    let mut tags = Vec::new();
    let mut rest = header;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix("W/") {
            rest = stripped.trim_start();
        }
        let Some(inner) = rest.strip_prefix('"') else {
            break;
        };
        let Some(end) = inner.find('"') else {
            break;
        };
        tags.push(inner[..end].to_string());
        rest = inner[end + 1..].trim_start();
        if let Some(stripped) = rest.strip_prefix(',') {
            rest = stripped;
        } else {
            break;
        }
    }
    tags
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
