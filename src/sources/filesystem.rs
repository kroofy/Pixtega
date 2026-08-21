//! Filesystem Source adapter.
//!
//! Resolves keys beneath a configured absolute root. Rejects every symlink
//! encountered below the root. A missing regular file is absence; a
//! directory or other non-regular file is Source unavailability. Enforces
//! the same byte limit as the HTTP(S) adapter.

use async_trait::async_trait;

use crate::config::FilesystemSourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, UpstreamKey};

pub struct FilesystemSource {
    // implemented by the source-adapter module
}

impl FilesystemSource {
    pub fn new(
        config: &FilesystemSourceConfig,
        limits: FetchLimits,
    ) -> Result<Self, ConfigError> {
        let _ = (config, limits);
        todo!("implemented by the source-adapter module")
    }
}

#[async_trait]
impl Source for FilesystemSource {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        let _ = key;
        todo!("implemented by the source-adapter module")
    }
}
