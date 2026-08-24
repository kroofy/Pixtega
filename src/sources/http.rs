//! HTTP(S) Source adapter.
//!
//! Joins base URL + Key Prefix + validated path with a URL parser. One
//! timeout covers the whole exchange. Redirects are bounded, same-origin,
//! and stay beneath the configured base path. Unless the source opts in
//! with `allow_private_destinations`, private and local destinations
//! (loopback, link-local, metadata-style hosts) are refused before any
//! connection is made — for the initial URL and every redirect hop. Sends
//! `Accept-Encoding: identity` and rejects any other content encoding.
//! 404/410 are absence; every other non-2xx is unavailability. Byte limits
//! are checked against `Content-Length` when present and enforced again
//! while streaming.

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, LOCATION};
use reqwest::StatusCode;
use url::Url;

use crate::config::HttpSourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, UpstreamKey};

pub struct HttpSource {
    /// Redirects are never followed by the client itself; the adapter
    /// follows them manually so origin and base-path rules can be enforced.
    client: reqwest::Client,
    base_url: Url,
    /// Canonical non-empty path segments of `base_url`, used for the
    /// base-path prefix check on redirect targets.
    base_path_segments: Vec<String>,
    /// When false (the default), every URL this adapter would fetch —
    /// the initial request and each redirect hop — is rejected before
    /// connecting if it targets a private or local destination.
    allow_private_destinations: bool,
    limits: FetchLimits,
}

impl HttpSource {
    pub fn new(config: &HttpSourceConfig, limits: FetchLimits) -> Result<Self, ConfigError> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .tls_backend_rustls()
            // Belt: per-request timeout; the adapter additionally wraps the
            // whole exchange (all hops + body) in one tokio timeout.
            .timeout(limits.timeout)
            .connect_timeout(limits.timeout);

        if let Some(ca_path) = &config.ca_certificate_file {
            let pem = std::fs::read(ca_path).map_err(|err| {
                ConfigError::new(format!(
                    "cannot read ca_certificate_file {}: {err}",
                    ca_path.display()
                ))
            })?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|err| {
                ConfigError::new(format!(
                    "cannot parse ca_certificate_file {}: {err}",
                    ca_path.display()
                ))
            })?;
            if certificates.is_empty() {
                return Err(ConfigError::new(format!(
                    "ca_certificate_file {} contains no certificates",
                    ca_path.display()
                )));
            }
            // Exclusive trust store for this source (fixture / private CAs).
            builder = builder.tls_certs_only(certificates);
        }

        let client = builder
            .build()
            .map_err(|err| ConfigError::new(format!("cannot build HTTP client: {err}")))?;

        Ok(HttpSource {
            client,
            base_url: config.base_url.clone(),
            base_path_segments: canonical_path_segments(&config.base_url),
            allow_private_destinations: config.allow_private_destinations,
            limits,
        })
    }

    async fn fetch_inner(&self, mut url: Url) -> Result<FetchedObject, SourceError> {
        let mut redirects_followed: u32 = 0;
        loop {
            // Belt: configuration validation already rejects private
            // base URLs without the opt-in; re-checking here keeps the
            // guarantee even for adapters constructed outside `load_from_*`
            // and covers every redirect hop before a connection is made.
            if !self.allow_private_destinations
                && crate::config::is_private_or_local_destination(&url)
            {
                return Err(unavailable(None, "destination blocked by policy"));
            }
            let response = self
                .client
                .get(url.clone())
                .header(ACCEPT_ENCODING, "identity")
                .send()
                .await
                .map_err(map_reqwest_error)?;

            let status = response.status();
            if is_followable_redirect(status) {
                redirects_followed += 1;
                if redirects_followed > self.limits.max_redirects {
                    return Err(unavailable(Some(status), "redirect limit exceeded"));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .ok_or_else(|| unavailable(Some(status), "redirect without Location"))?;
                let location = location
                    .to_str()
                    .map_err(|_| unavailable(Some(status), "invalid Location header"))?;
                // Relative Locations resolve against the current URL.
                let next = url
                    .join(location)
                    .map_err(|_| unavailable(Some(status), "unparseable Location header"))?;
                self.check_redirect_target(&next)?;
                url = next;
                continue;
            }

            let code = status.as_u16();
            if code == 404 || code == 410 {
                return Err(SourceError::NotFound {
                    upstream_status: Some(code),
                });
            }
            if !status.is_success() {
                return Err(unavailable(Some(status), "unexpected upstream status"));
            }

            if let Some(encoding) = response.headers().get(CONTENT_ENCODING) {
                let is_identity = encoding
                    .to_str()
                    .map(|value| value.trim().eq_ignore_ascii_case("identity"))
                    .unwrap_or(false);
                if !is_identity {
                    return Err(unavailable(Some(status), "unsupported content encoding"));
                }
            }

            // Advertised length check; the header may be absent or false,
            // so the limit is enforced again below while streaming.
            let advertised = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
            if let Some(advertised) = advertised {
                if advertised > self.limits.max_bytes {
                    return Err(SourceError::TooLarge {
                        upstream_status: Some(code),
                    });
                }
            }

            // Preallocate from the advertised length (already bounded by
            // max_bytes above) so large bodies avoid the repeated
            // grow-and-copy of an amortized Vec.
            let mut bytes: Vec<u8> = Vec::with_capacity(advertised.unwrap_or(0) as usize);
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(map_reqwest_error)?;
                if bytes.len() as u64 + chunk.len() as u64 > self.limits.max_bytes {
                    return Err(SourceError::TooLarge {
                        upstream_status: Some(code),
                    });
                }
                bytes.extend_from_slice(&chunk);
            }

            return Ok(FetchedObject {
                bytes,
                upstream_status: Some(code),
            });
        }
    }

    /// A redirect target must keep the configured scheme, host, and
    /// effective port, and must stay beneath the configured base path.
    fn check_redirect_target(&self, next: &Url) -> Result<(), SourceError> {
        let base = &self.base_url;
        if next.scheme() != base.scheme()
            || next.host_str() != base.host_str()
            || next.port_or_known_default() != base.port_or_known_default()
        {
            return Err(unavailable(None, "redirect left the configured origin"));
        }
        let next_segments = canonical_path_segments(next);
        let beneath_base = next_segments.len() >= self.base_path_segments.len()
            && next_segments
                .iter()
                .zip(self.base_path_segments.iter())
                .all(|(next_segment, base_segment)| next_segment == base_segment);
        if !beneath_base {
            return Err(unavailable(
                None,
                "redirect escaped the configured base path",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl Source for HttpSource {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| unavailable(None, "base URL cannot carry path segments"))?;
            segments.pop_if_empty();
            segments.extend(&key.segments);
        }
        // One timeout for the whole exchange: every redirect hop and the
        // complete body stream.
        match tokio::time::timeout(self.limits.timeout, self.fetch_inner(url)).await {
            Ok(result) => result,
            Err(_) => Err(SourceError::Timeout),
        }
    }
}

fn is_followable_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

fn unavailable(status: Option<StatusCode>, detail: &str) -> SourceError {
    SourceError::Unavailable {
        upstream_status: status.map(|s| s.as_u16()),
        detail: detail.to_string(),
    }
}

/// Terse mapping of client errors. Never includes response bodies or
/// credentials.
fn map_reqwest_error(err: reqwest::Error) -> SourceError {
    if err.is_timeout() {
        return SourceError::Timeout;
    }
    let detail = if err.is_connect() {
        "connection to upstream failed"
    } else if err.is_body() || err.is_decode() {
        "upstream body read failed"
    } else {
        "upstream request failed"
    };
    SourceError::Unavailable {
        upstream_status: err.status().map(|s| s.as_u16()),
        detail: detail.to_string(),
    }
}

/// Non-empty path segments as parsed by the URL crate (which normalizes
/// during parsing), so base-path comparison works on canonical segments.
fn canonical_path_segments(url: &Url) -> Vec<String> {
    url.path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
