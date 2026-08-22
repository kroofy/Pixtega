//! The internal Source seam.
//!
//! The Image Service depends only on [`Source`]; HTTP(S), filesystem, and S3
//! are adapters behind it. Request parsing and image processing contain no
//! transport-specific branches.

pub mod filesystem;
pub mod http;
pub mod s3;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::{AppConfig, SourceConfig};
use crate::errors::{ConfigError, SourceError};
use crate::types::{FetchedObject, UpstreamKey};

/// Limits every adapter enforces while fetching.
#[derive(Debug, Clone, Copy)]
pub struct FetchLimits {
    /// Maximum Source Object size in bytes. Enforced against an advertised
    /// length when one exists AND again while streaming the body.
    pub max_bytes: u64,
    /// One timeout applied to the whole exchange, including body streaming.
    pub timeout: Duration,
    /// Maximum number of redirects an HTTP(S) fetch may follow.
    pub max_redirects: u32,
}

/// A configured location the service may read a Source Object from.
#[async_trait]
pub trait Source: Send + Sync {
    /// Fetch the Source Object at `key` (Key Prefix + validated source path).
    ///
    /// Adapters must map their transport's failures onto the shared
    /// [`SourceError`] taxonomy: absence is `NotFound`, permission denial is
    /// `Unavailable` (never `NotFound`), size violations are `TooLarge`,
    /// and timeouts are `Timeout`.
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError>;
}

/// Maps Mounts to constructed Source adapters.
pub struct SourceRegistry {
    sources: HashMap<String, Arc<dyn Source>>,
}

impl SourceRegistry {
    /// Build every adapter from validated configuration. Construction
    /// failures (for example an unreadable CA file) stop startup.
    pub async fn from_config(config: &AppConfig) -> Result<Self, ConfigError> {
        let limits = FetchLimits {
            max_bytes: config.max_download_bytes,
            timeout: Duration::from_millis(config.download_timeout_ms),
            max_redirects: config.max_redirects,
        };
        let mut sources: HashMap<String, Arc<dyn Source>> = HashMap::new();
        for source in &config.sources {
            let adapter: Arc<dyn Source> = match source {
                SourceConfig::Http(c) => Arc::new(http::HttpSource::new(c, limits)?),
                SourceConfig::Filesystem(c) => {
                    Arc::new(filesystem::FilesystemSource::new(c, limits)?)
                }
                SourceConfig::S3(c) => Arc::new(s3::S3Source::new(c, limits).await?),
            };
            sources.insert(source.mount().to_string(), adapter);
        }
        Ok(SourceRegistry { sources })
    }

    pub fn get(&self, mount: &str) -> Option<Arc<dyn Source>> {
        self.sources.get(mount).cloned()
    }
}
