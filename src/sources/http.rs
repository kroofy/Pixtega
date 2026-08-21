//! HTTP(S) Source adapter.
//!
//! Joins base URL + Key Prefix + validated path with a URL parser. One
//! timeout covers the whole exchange. Redirects are bounded, same-origin,
//! and stay beneath the configured base path. Sends
//! `Accept-Encoding: identity` and rejects any other content encoding.
//! 404/410 are absence; every other non-2xx is unavailability. Byte limits
//! are checked against `Content-Length` when present and enforced again
//! while streaming.

use async_trait::async_trait;

use crate::config::HttpSourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, UpstreamKey};

pub struct HttpSource {
    // implemented by the source-adapter module
}

impl HttpSource {
    pub fn new(config: &HttpSourceConfig, limits: FetchLimits) -> Result<Self, ConfigError> {
        let _ = (config, limits);
        todo!("implemented by the source-adapter module")
    }
}

#[async_trait]
impl Source for HttpSource {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        let _ = key;
        todo!("implemented by the source-adapter module")
    }
}
