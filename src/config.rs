//! TOML configuration: schema, parsing, and validation.
//!
//! The schema is closed: unknown fields are startup errors. Every rule in
//! SPEC.md "Configuration" is enforced here so the rest of the application
//! can trust an [`AppConfig`] completely.
//!
//! Relative filesystem roots and CA certificate paths are resolved against
//! the directory containing the configuration file, then canonicalized once
//! at startup.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use url::Url;

use crate::errors::ConfigError;
use crate::types::OutputFormat;

/// Fully validated application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub listen_address: SocketAddr,
    /// Starts with `/`, canonical non-empty segments, no trailing slash.
    /// Defaults to `/images`.
    pub path_prefix: String,
    /// The Width Allowlist: deduplicated, validated, in `1..=16384`.
    pub allowed_widths: Vec<u32>,
    pub max_download_bytes: u64,
    pub max_source_megapixels: u64,
    pub download_timeout_ms: u64,
    pub max_redirects: u32,
    pub max_concurrent_derivations: usize,
    pub unversioned_success_ttl_seconds: u64,
    pub not_found_ttl_seconds: u64,
    /// Per-format policies. Only formats present here are enabled.
    pub formats: BTreeMap<OutputFormat, FormatPolicy>,
    pub sources: Vec<SourceConfig>,
}

impl AppConfig {
    /// The policy for a format, if that format is enabled.
    pub fn format_policy(&self, format: OutputFormat) -> Option<&FormatPolicy> {
        self.formats.get(&format)
    }

    /// The Source configuration selected by a Mount, if any.
    pub fn source_for_mount(&self, mount: &str) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.mount() == mount)
    }
}

/// Encoder policy for one output format. Quality scales are not comparable
/// across codecs, so each format owns its default and allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatPolicy {
    /// Required; in `1..=100`.
    pub default_quality: u32,
    /// May be empty: callers cannot then specify `q` for this format.
    /// Never contains the default quality or duplicates.
    pub allowed_qualities: Vec<u32>,
}

/// One validated Source. Transport-specific fields are only representable
/// for their own transport.
#[derive(Debug, Clone)]
pub enum SourceConfig {
    Http(HttpSourceConfig),
    Filesystem(FilesystemSourceConfig),
    S3(S3SourceConfig),
}

impl SourceConfig {
    pub fn mount(&self) -> &str {
        match self {
            SourceConfig::Http(c) => &c.mount,
            SourceConfig::Filesystem(c) => &c.mount,
            SourceConfig::S3(c) => &c.mount,
        }
    }

    /// Key Prefix as validated segments. Empty when `key_prefix = ""`.
    /// Defaults to `[mount]` when omitted in TOML.
    pub fn key_prefix_segments(&self) -> &[String] {
        match self {
            SourceConfig::Http(c) => &c.key_prefix_segments,
            SourceConfig::Filesystem(c) => &c.key_prefix_segments,
            SourceConfig::S3(c) => &c.key_prefix_segments,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpSourceConfig {
    pub mount: String,
    pub key_prefix_segments: Vec<String>,
    /// HTTP or HTTPS.
    pub base_url: Url,
    /// PEM CA bundle for local fixture servers; only valid with an HTTPS
    /// base URL. Resolved relative to the configuration file. TLS hostname
    /// verification stays enabled.
    pub ca_certificate_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FilesystemSourceConfig {
    pub mount: String,
    pub key_prefix_segments: Vec<String>,
    /// Absolute, canonicalized at startup, verified readable directory.
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct S3SourceConfig {
    pub mount: String,
    pub key_prefix_segments: Vec<String>,
    pub bucket: String,
    pub region: String,
    /// HTTP or HTTPS; for local S3-compatible test servers.
    pub endpoint_url: Option<Url>,
    pub force_path_style: bool,
}

/// Load and validate configuration from a TOML file.
///
/// `path` is the file passed on the command line or through `CONFIG_FILE`.
/// Relative paths inside the file resolve against the file's directory.
pub fn load_from_file(path: &Path) -> Result<AppConfig, ConfigError> {
    let _ = path;
    todo!("implemented by the configuration module")
}

/// Load and validate configuration from an inline TOML string (`CONFIG`).
///
/// `base_dir` is the directory relative paths resolve against.
pub fn load_from_str(toml_text: &str, base_dir: &Path) -> Result<AppConfig, ConfigError> {
    let _ = (toml_text, base_dir);
    todo!("implemented by the configuration module")
}
