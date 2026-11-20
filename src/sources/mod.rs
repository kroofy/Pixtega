//! The internal Source seam.
//!
//! The Image Service depends only on [`Source`]; HTTP(S), filesystem, and S3
//! are adapters behind it. Request parsing and image processing contain no
//! transport-specific branches.

pub mod filesystem;
pub mod http;
pub mod s3;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::{AppConfig, SourceConfig};
use crate::errors::{ConfigError, SourceError};
use crate::types::{FetchedObject, IdentifiedObject, UpstreamKey};

/// Limits every adapter enforces while fetching.
#[derive(Debug, Clone, Copy)]
pub struct FetchLimits {
    /// Maximum Source Object size in bytes. Enforced against an advertised
    /// length when one exists ([`FetchLimits::admit`]) AND again while
    /// streaming the body ([`BoundedBody`]).
    pub max_bytes: u64,
    /// One timeout applied to the whole exchange, including body streaming
    /// ([`FetchLimits::within`]).
    pub timeout: Duration,
    /// Maximum number of redirects an HTTP(S) fetch may follow.
    pub max_redirects: u32,
}

impl FetchLimits {
    /// Admit a Source Object by the length its Transport advertised, before
    /// any of it is read. `upstream_status` is the Transport's status for
    /// the exchange, reported with the refusal.
    pub fn admit(&self, advertised: u64, upstream_status: Option<u16>) -> Result<(), SourceError> {
        if advertised > self.max_bytes {
            return Err(SourceError::TooLarge { upstream_status });
        }
        Ok(())
    }

    /// Run one whole exchange with a Source — every hop, every retry, and
    /// the complete body — under the download timeout. This is the only
    /// deadline on an exchange: the clients beneath the adapters are given
    /// no request or operation timeout of their own.
    pub async fn within<T>(
        &self,
        exchange: impl Future<Output = Result<T, SourceError>>,
    ) -> Result<T, SourceError> {
        tokio::time::timeout(self.timeout, exchange)
            .await
            .unwrap_or(Err(SourceError::Timeout))
    }

    /// Start reading a Source Object body under the byte limit.
    pub fn body(
        &self,
        advertised: Option<u64>,
        upstream_status: Option<u16>,
    ) -> Result<BoundedBody, SourceError> {
        if let Some(advertised) = advertised {
            self.admit(advertised, upstream_status)?;
        }
        Ok(BoundedBody {
            // Sized once from the advertised length (just admitted, so at
            // most `max_bytes`): a large body is not grown and re-copied
            // on the way in.
            bytes: Vec::with_capacity(advertised.unwrap_or(0) as usize),
            max_bytes: self.max_bytes,
            upstream_status,
        })
    }
}

/// A Source Object body being read, and the one place the byte limit is
/// enforced on what a Transport actually delivers. An advertised length is
/// a hint, not a promise, so every chunk is checked.
pub struct BoundedBody {
    bytes: Vec<u8>,
    max_bytes: u64,
    upstream_status: Option<u16>,
}

impl BoundedBody {
    /// Append one chunk, refusing the body once it would exceed the limit.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), SourceError> {
        if self.bytes.len() as u64 + chunk.len() as u64 > self.max_bytes {
            return Err(SourceError::TooLarge {
                upstream_status: self.upstream_status,
            });
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
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

    /// Resolve object identity without reading bytes, for conditional
    /// requests. `Ok(None)` means the Transport has no validator (HTTP HEAD
    /// unsupported or no `ETag`); the caller must fetch. Failures use the
    /// same taxonomy as [`Source::fetch`].
    async fn identify(&self, key: &UpstreamKey) -> Result<Option<IdentifiedObject>, SourceError>;
}

/// The constructed adapter of every configured Source, in configuration
/// order: a request names its Source by position
/// ([`ResolvedRequest::source`](crate::types::ResolvedRequest::source)).
pub struct SourceRegistry {
    sources: Vec<Arc<dyn Source>>,
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
        let mut sources: Vec<Arc<dyn Source>> = Vec::with_capacity(config.sources.len());
        for source in &config.sources {
            sources.push(match source {
                SourceConfig::Http(c) => Arc::new(http::HttpSource::new(c, limits)?),
                SourceConfig::Filesystem(c) => {
                    Arc::new(filesystem::FilesystemSource::new(c, limits)?)
                }
                SourceConfig::S3(c) => Arc::new(s3::S3Source::new(c, limits).await?),
            });
        }
        Ok(SourceRegistry { sources })
    }

    /// The adapter of the Source at `index` in the configuration this
    /// registry was built from.
    pub fn source(&self, index: usize) -> &dyn Source {
        self.sources[index].as_ref()
    }
}
