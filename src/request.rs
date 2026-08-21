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

use crate::config::AppConfig;
use crate::errors::RequestError;
use crate::types::ResolvedRequest;

/// Maximum accepted request-target length in bytes (path + `?` + query).
pub const MAX_TARGET_BYTES: usize = 8192;

/// Parse and validate one GET request target.
///
/// `raw_path` is the raw, still-percent-encoded URI path. `raw_query` is
/// the raw query string without the leading `?`, when one exists.
///
/// On success the returned [`ResolvedRequest`] carries the Mount, the
/// upstream key (Key Prefix applied), the resolved Transform, and whether
/// the request was versioned. The Source is guaranteed to be configured.
pub fn parse_request(
    config: &AppConfig,
    raw_path: &str,
    raw_query: Option<&str>,
) -> Result<ResolvedRequest, RequestError> {
    let _ = (config, raw_path, raw_query);
    todo!("implemented by the request-parsing module")
}
