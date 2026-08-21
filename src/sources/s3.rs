//! S3 Source adapter.
//!
//! Reads from a configured bucket/region with the standard SDK credential
//! provider chain; credentials never come from TOML. Supports optional
//! `endpoint_url` and `force_path_style` for local S3-compatible fixtures.
//! The request `v` parameter is never sent as an S3 `versionId` and never
//! becomes part of the object key. Missing-key responses map to absence;
//! access denial maps to unavailability; SDK timeouts map to the shared
//! timeout outcome. The advertised object length and the streamed body are
//! both checked against the byte limit.

use async_trait::async_trait;

use crate::config::S3SourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, UpstreamKey};

pub struct S3Source {
    // implemented by the source-adapter module
}

impl S3Source {
    pub async fn new(config: &S3SourceConfig, limits: FetchLimits) -> Result<Self, ConfigError> {
        let _ = (config, limits);
        todo!("implemented by the source-adapter module")
    }
}

#[async_trait]
impl Source for S3Source {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        let _ = key;
        todo!("implemented by the source-adapter module")
    }
}
