//! TOML configuration: schema, parsing, and validation.
//!
//! The schema is closed: unknown fields are startup errors. Every rule in
//! SPEC.md "Configuration" is enforced here so the rest of the application
//! can trust an [`AppConfig`] completely.
//!
//! Relative filesystem roots and CA certificate paths are resolved against
//! the directory containing the configuration file, then canonicalized once
//! at startup.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
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
    let toml_text = std::fs::read_to_string(path).map_err(|e| {
        ConfigError::new(format!(
            "cannot read configuration file `{}`: {e}",
            path.display()
        ))
    })?;
    let base_dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    load_from_str(&toml_text, &base_dir)
}

/// Load and validate configuration from an inline TOML string (`CONFIG`).
///
/// `base_dir` is the directory relative paths resolve against.
pub fn load_from_str(toml_text: &str, base_dir: &Path) -> Result<AppConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(toml_text)
        .map_err(|e| ConfigError::new(format!("invalid configuration: {e}")))?;
    validate(raw, base_dir)
}

// ---------------------------------------------------------------------------
// Raw deserialization schema.
//
// The schema is closed: `deny_unknown_fields` on every raw struct turns any
// unrecognized TOML key (including credential fields) into a startup error.
// Sources are deserialized as one flat struct with every transport-specific
// field optional, because serde's internally tagged enums do not honor
// `deny_unknown_fields`; transport-field exclusivity is validated manually.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    listen_address: String,
    path_prefix: Option<String>,
    allowed_widths: Vec<i64>,
    max_download_bytes: i64,
    max_source_megapixels: i64,
    download_timeout_ms: i64,
    max_redirects: i64,
    max_concurrent_derivations: i64,
    unversioned_success_ttl_seconds: i64,
    not_found_ttl_seconds: i64,
    formats: BTreeMap<String, RawFormatPolicy>,
    sources: Vec<RawSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFormatPolicy {
    default_quality: i64,
    allowed_qualities: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    mount: String,
    transport: String,
    key_prefix: Option<String>,
    // HTTP(S)-only fields.
    base_url: Option<String>,
    ca_certificate_file: Option<String>,
    // Filesystem-only fields.
    root: Option<String>,
    // S3-only fields.
    bucket: Option<String>,
    region: Option<String>,
    endpoint_url: Option<String>,
    force_path_style: Option<bool>,
}

// ---------------------------------------------------------------------------
// Validation.
// ---------------------------------------------------------------------------

fn validate(raw: RawConfig, base_dir: &Path) -> Result<AppConfig, ConfigError> {
    let listen_address: SocketAddr = raw.listen_address.parse().map_err(|e| {
        ConfigError::new(format!(
            "`listen_address` `{}` is not a valid socket address: {e}",
            raw.listen_address
        ))
    })?;

    let path_prefix = validate_path_prefix(raw.path_prefix.as_deref().unwrap_or("/images"))?;
    let allowed_widths = validate_widths(&raw.allowed_widths)?;

    let max_download_bytes =
        check_range("max_download_bytes", raw.max_download_bytes, 1, 104_857_600)? as u64;
    let max_source_megapixels =
        check_range("max_source_megapixels", raw.max_source_megapixels, 1, 500)? as u64;
    let download_timeout_ms =
        check_range("download_timeout_ms", raw.download_timeout_ms, 1, 60_000)? as u64;
    let max_redirects = check_range("max_redirects", raw.max_redirects, 0, 10)? as u32;
    let max_concurrent_derivations = check_range(
        "max_concurrent_derivations",
        raw.max_concurrent_derivations,
        1,
        64,
    )? as usize;
    let unversioned_success_ttl_seconds = check_range(
        "unversioned_success_ttl_seconds",
        raw.unversioned_success_ttl_seconds,
        1,
        86_400,
    )? as u64;
    let not_found_ttl_seconds =
        check_range("not_found_ttl_seconds", raw.not_found_ttl_seconds, 1, 3_600)? as u64;

    let formats = validate_formats(&raw.formats)?;
    let sources = validate_sources(&raw.sources, base_dir)?;

    Ok(AppConfig {
        listen_address,
        path_prefix,
        allowed_widths,
        max_download_bytes,
        max_source_megapixels,
        download_timeout_ms,
        max_redirects,
        max_concurrent_derivations,
        unversioned_success_ttl_seconds,
        not_found_ttl_seconds,
        formats,
        sources,
    })
}

fn check_range(name: &str, value: i64, min: i64, max: i64) -> Result<i64, ConfigError> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(ConfigError::new(format!(
            "`{name}` must be in {min}..={max}, got {value}"
        )))
    }
}

fn validate_path_prefix(prefix: &str) -> Result<String, ConfigError> {
    if !prefix.starts_with('/') {
        return Err(ConfigError::new(format!(
            "`path_prefix` `{prefix}` must start with `/`"
        )));
    }
    if prefix.ends_with('/') {
        return Err(ConfigError::new(format!(
            "`path_prefix` `{prefix}` must not end with `/`"
        )));
    }
    for segment in prefix[1..].split('/') {
        if segment.is_empty() {
            return Err(ConfigError::new(format!(
                "`path_prefix` `{prefix}` contains an empty segment"
            )));
        }
        if segment == "." || segment == ".." {
            return Err(ConfigError::new(format!(
                "`path_prefix` `{prefix}` contains a `{segment}` segment"
            )));
        }
    }
    Ok(prefix.to_string())
}

fn validate_widths(raw_widths: &[i64]) -> Result<Vec<u32>, ConfigError> {
    if raw_widths.is_empty() {
        return Err(ConfigError::new(
            "`allowed_widths` must contain at least one width",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut widths = Vec::with_capacity(raw_widths.len());
    for &width in raw_widths {
        if !(1..=16_384).contains(&width) {
            return Err(ConfigError::new(format!(
                "`allowed_widths` contains width {width}; widths must be in 1..=16384"
            )));
        }
        if !seen.insert(width) {
            return Err(ConfigError::new(format!(
                "`allowed_widths` contains duplicate width {width}"
            )));
        }
        widths.push(width as u32);
    }
    Ok(widths)
}

fn validate_formats(
    raw_formats: &BTreeMap<String, RawFormatPolicy>,
) -> Result<BTreeMap<OutputFormat, FormatPolicy>, ConfigError> {
    if raw_formats.is_empty() {
        return Err(ConfigError::new(
            "`formats` must define at least one format policy",
        ));
    }
    let mut formats = BTreeMap::new();
    for (name, raw_policy) in raw_formats {
        let format = OutputFormat::from_canonical(name).ok_or_else(|| {
            ConfigError::new(format!(
                "unknown format policy `formats.{name}`; expected `webp`, `avif`, or `jpeg`"
            ))
        })?;
        let default_quality = check_range(
            &format!("formats.{name}.default_quality"),
            raw_policy.default_quality,
            1,
            100,
        )? as u32;
        let mut seen = BTreeSet::new();
        let mut allowed_qualities = Vec::new();
        for &quality in raw_policy.allowed_qualities.as_deref().unwrap_or_default() {
            let quality = check_range(
                &format!("formats.{name}.allowed_qualities"),
                quality,
                1,
                100,
            )? as u32;
            if quality == default_quality {
                return Err(ConfigError::new(format!(
                    "`formats.{name}.allowed_qualities` contains {quality}, which equals \
                     `default_quality`; omitting `q` is the canonical spelling of the default"
                )));
            }
            if !seen.insert(quality) {
                return Err(ConfigError::new(format!(
                    "`formats.{name}.allowed_qualities` contains duplicate quality {quality}"
                )));
            }
            allowed_qualities.push(quality);
        }
        formats.insert(
            format,
            FormatPolicy {
                default_quality,
                allowed_qualities,
            },
        );
    }
    Ok(formats)
}

fn validate_sources(
    raw_sources: &[RawSource],
    base_dir: &Path,
) -> Result<Vec<SourceConfig>, ConfigError> {
    if raw_sources.is_empty() {
        return Err(ConfigError::new(
            "`sources` must define at least one source",
        ));
    }
    let mut seen_mounts = BTreeSet::new();
    let mut sources = Vec::with_capacity(raw_sources.len());
    for raw in raw_sources {
        validate_mount(&raw.mount)?;
        if !seen_mounts.insert(raw.mount.clone()) {
            return Err(ConfigError::new(format!(
                "duplicate mount `{}`; mount names must be unique",
                raw.mount
            )));
        }
        sources.push(validate_source(raw, base_dir)?);
    }
    Ok(sources)
}

fn validate_mount(mount: &str) -> Result<(), ConfigError> {
    let bytes = mount.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if valid {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "invalid mount `{mount}`: mount names must match [a-z][a-z0-9-]{{0,31}}"
        )))
    }
}

fn validate_source(raw: &RawSource, base_dir: &Path) -> Result<SourceConfig, ConfigError> {
    let mount = &raw.mount;
    let key_prefix_segments = validate_key_prefix(mount, raw.key_prefix.as_deref())?;

    match raw.transport.as_str() {
        "http" => {
            forbid_field(mount, "http", "root", raw.root.is_some())?;
            forbid_field(mount, "http", "bucket", raw.bucket.is_some())?;
            forbid_field(mount, "http", "region", raw.region.is_some())?;
            forbid_field(mount, "http", "endpoint_url", raw.endpoint_url.is_some())?;
            forbid_field(
                mount,
                "http",
                "force_path_style",
                raw.force_path_style.is_some(),
            )?;
            let base_url = raw.base_url.as_deref().ok_or_else(|| {
                ConfigError::new(format!(
                    "source `{mount}`: transport `http` requires `base_url`"
                ))
            })?;
            let base_url = parse_http_url(mount, "base_url", base_url)?;
            let ca_certificate_file = match raw.ca_certificate_file.as_deref() {
                None => None,
                Some(ca_path) => {
                    if base_url.scheme() != "https" {
                        return Err(ConfigError::new(format!(
                            "source `{mount}`: `ca_certificate_file` is only valid when \
                             `base_url` uses https"
                        )));
                    }
                    Some(resolve_ca_certificate_file(mount, ca_path, base_dir)?)
                }
            };
            Ok(SourceConfig::Http(HttpSourceConfig {
                mount: mount.clone(),
                key_prefix_segments,
                base_url,
                ca_certificate_file,
            }))
        }
        "filesystem" => {
            forbid_field(mount, "filesystem", "base_url", raw.base_url.is_some())?;
            forbid_field(
                mount,
                "filesystem",
                "ca_certificate_file",
                raw.ca_certificate_file.is_some(),
            )?;
            forbid_field(mount, "filesystem", "bucket", raw.bucket.is_some())?;
            forbid_field(mount, "filesystem", "region", raw.region.is_some())?;
            forbid_field(
                mount,
                "filesystem",
                "endpoint_url",
                raw.endpoint_url.is_some(),
            )?;
            forbid_field(
                mount,
                "filesystem",
                "force_path_style",
                raw.force_path_style.is_some(),
            )?;
            let root = raw.root.as_deref().ok_or_else(|| {
                ConfigError::new(format!(
                    "source `{mount}`: transport `filesystem` requires `root`"
                ))
            })?;
            let root = resolve_filesystem_root(mount, root, base_dir)?;
            Ok(SourceConfig::Filesystem(FilesystemSourceConfig {
                mount: mount.clone(),
                key_prefix_segments,
                root,
            }))
        }
        "s3" => {
            forbid_field(mount, "s3", "base_url", raw.base_url.is_some())?;
            forbid_field(
                mount,
                "s3",
                "ca_certificate_file",
                raw.ca_certificate_file.is_some(),
            )?;
            forbid_field(mount, "s3", "root", raw.root.is_some())?;
            let bucket = raw.bucket.as_deref().ok_or_else(|| {
                ConfigError::new(format!(
                    "source `{mount}`: transport `s3` requires `bucket`"
                ))
            })?;
            validate_s3_name(mount, "bucket", bucket)?;
            let region = raw.region.as_deref().ok_or_else(|| {
                ConfigError::new(format!(
                    "source `{mount}`: transport `s3` requires `region`"
                ))
            })?;
            validate_s3_name(mount, "region", region)?;
            let endpoint_url = raw
                .endpoint_url
                .as_deref()
                .map(|u| parse_http_url(mount, "endpoint_url", u))
                .transpose()?;
            Ok(SourceConfig::S3(S3SourceConfig {
                mount: mount.clone(),
                key_prefix_segments,
                bucket: bucket.to_string(),
                region: region.to_string(),
                endpoint_url,
                force_path_style: raw.force_path_style.unwrap_or(false),
            }))
        }
        other => Err(ConfigError::new(format!(
            "source `{mount}`: `transport` must be `http`, `filesystem`, or `s3`, got `{other}`"
        ))),
    }
}

fn forbid_field(
    mount: &str,
    transport: &str,
    field: &str,
    present: bool,
) -> Result<(), ConfigError> {
    if present {
        Err(ConfigError::new(format!(
            "source `{mount}`: `{field}` is not valid for transport `{transport}`"
        )))
    } else {
        Ok(())
    }
}

/// Omitted => one segment equal to the mount. Empty string => no segments.
/// Otherwise a relative path with no empty, `.` or `..` segment.
fn validate_key_prefix(mount: &str, key_prefix: Option<&str>) -> Result<Vec<String>, ConfigError> {
    match key_prefix {
        None => Ok(vec![mount.to_string()]),
        Some("") => Ok(Vec::new()),
        Some(prefix) => {
            let segments: Vec<String> = prefix.split('/').map(str::to_string).collect();
            for segment in &segments {
                if segment.is_empty() {
                    return Err(ConfigError::new(format!(
                        "source `{mount}`: `key_prefix` `{prefix}` must be a relative path \
                         without empty segments"
                    )));
                }
                if segment == "." || segment == ".." {
                    return Err(ConfigError::new(format!(
                        "source `{mount}`: `key_prefix` `{prefix}` contains a traversal \
                         segment `{segment}`"
                    )));
                }
            }
            Ok(segments)
        }
    }
}

fn parse_http_url(mount: &str, field: &str, value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|e| {
        ConfigError::new(format!(
            "source `{mount}`: `{field}` `{value}` is not a valid URL: {e}"
        ))
    })?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(ConfigError::new(format!(
            "source `{mount}`: `{field}` scheme must be http or https, got `{other}`"
        ))),
    }
}

fn resolve_ca_certificate_file(
    mount: &str,
    ca_path: &str,
    base_dir: &Path,
) -> Result<PathBuf, ConfigError> {
    let resolved = base_dir.join(ca_path);
    let metadata = std::fs::metadata(&resolved).map_err(|e| {
        ConfigError::new(format!(
            "source `{mount}`: `ca_certificate_file` `{}` is not readable: {e}",
            resolved.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(ConfigError::new(format!(
            "source `{mount}`: `ca_certificate_file` `{}` is not a regular file",
            resolved.display()
        )));
    }
    std::fs::File::open(&resolved).map_err(|e| {
        ConfigError::new(format!(
            "source `{mount}`: `ca_certificate_file` `{}` cannot be opened: {e}",
            resolved.display()
        ))
    })?;
    Ok(resolved)
}

fn resolve_filesystem_root(
    mount: &str,
    root: &str,
    base_dir: &Path,
) -> Result<PathBuf, ConfigError> {
    let resolved = base_dir.join(root);
    let canonical = std::fs::canonicalize(&resolved).map_err(|e| {
        ConfigError::new(format!(
            "source `{mount}`: `root` `{}` does not resolve to a readable directory: {e}",
            resolved.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(ConfigError::new(format!(
            "source `{mount}`: `root` `{}` is not a directory",
            canonical.display()
        )));
    }
    std::fs::read_dir(&canonical).map_err(|e| {
        ConfigError::new(format!(
            "source `{mount}`: `root` `{}` is not readable: {e}",
            canonical.display()
        ))
    })?;
    Ok(canonical)
}

fn validate_s3_name(mount: &str, field: &str, value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::new(format!(
            "source `{mount}`: `{field}` must not be empty"
        )));
    }
    if value.contains('/') {
        return Err(ConfigError::new(format!(
            "source `{mount}`: `{field}` `{value}` must not contain a slash"
        )));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ConfigError::new(format!(
            "source `{mount}`: `{field}` must not contain control characters"
        )));
    }
    Ok(())
}
