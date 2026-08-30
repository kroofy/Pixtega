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
use aws_sdk_s3::config::interceptors::InterceptorContext;
use aws_sdk_s3::config::retry::{ClassifyRetry, RetryAction, RetryConfig};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};

use crate::config::S3SourceConfig;
use crate::errors::{ConfigError, SourceError};
use crate::sources::{FetchLimits, Source};
use crate::types::{FetchedObject, IdentifiedObject, ObjectIdentity, UpstreamKey};

/// Retry `DispatchFailure` that never got an HTTP response, except
/// timeouts. Service errors stay one-shot: a 403 is still a 403.
///
/// `RetryForbidden` on every other failure overrides the SDK's default
/// classifiers (5xx, modeled retryable codes) if they are still installed.
#[derive(Debug, Default)]
struct TransportRetryClassifier;

impl ClassifyRetry for TransportRetryClassifier {
    fn classify_retry(&self, ctx: &InterceptorContext) -> RetryAction {
        let Some(Err(error)) = ctx.output_or_error() else {
            return RetryAction::NoActionIndicated;
        };
        if let Some(connector) = error.as_connector_error() {
            if !connector.is_timeout() {
                return RetryAction::transient_error();
            }
        }
        RetryAction::RetryForbidden
    }

    fn name(&self) -> &'static str {
        "s3 transport-only"
    }
}

pub struct S3Source {
    client: aws_sdk_s3::Client,
    bucket: String,
    limits: FetchLimits,
}

impl S3Source {
    pub async fn new(config: &S3SourceConfig, limits: FetchLimits) -> Result<Self, ConfigError> {
        // Standard credential provider chain (environment, profile, IMDS...).
        // Credentials are never read from service configuration.
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(config.region.clone()));
        if let Some(endpoint_url) = &config.endpoint_url {
            loader = loader.endpoint_url(endpoint_url.as_str());
        }
        let shared_config = loader.load().await;

        let timeout_config = aws_sdk_s3::config::timeout::TimeoutConfig::builder()
            .operation_timeout(limits.timeout)
            .operation_attempt_timeout(limits.timeout)
            .build();
        let s3_config = aws_sdk_s3::config::Builder::from(&shared_config)
            .force_path_style(config.force_path_style)
            .timeout_config(timeout_config)
            // Standard retry machinery (attempts, backoff) with a
            // transport-only classifier. Service errors stay one-shot.
            // `operation_timeout` already includes retries; `Source::fetch`
            // wraps the same budget again.
            .retry_config(RetryConfig::standard())
            .retry_classifier(TransportRetryClassifier)
            .build();

        Ok(S3Source {
            client: aws_sdk_s3::Client::from_conf(s3_config),
            bucket: config.bucket.clone(),
            limits,
        })
    }

    async fn fetch_inner(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        // The object key is exactly Key Prefix + validated source path.
        // No version_id is ever sent: the request `v` parameter is a cache
        // token, not an S3 object version.
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key.joined())
            .send()
            .await
            .map_err(map_sdk_error)?;

        // Advertised length check; enforced again below while streaming
        // because the transport may deliver more than it advertised.
        if let Some(advertised) = output.content_length() {
            if advertised < 0 || advertised as u64 > self.limits.max_bytes {
                return Err(SourceError::TooLarge {
                    upstream_status: Some(200),
                });
            }
        }

        let identity = s3_identity(output.e_tag(), output.version_id());
        let mut body = output.body;
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            match body.try_next().await {
                Ok(Some(chunk)) => {
                    if bytes.len() as u64 + chunk.len() as u64 > self.limits.max_bytes {
                        return Err(SourceError::TooLarge {
                            upstream_status: Some(200),
                        });
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => {
                    return Err(SourceError::Unavailable {
                        upstream_status: Some(200),
                        detail: "s3 body stream failed".to_string(),
                    });
                }
            }
        }

        Ok(FetchedObject {
            bytes,
            upstream_status: Some(200),
            identity,
        })
    }

    async fn identify_inner(
        &self,
        key: &UpstreamKey,
    ) -> Result<Option<IdentifiedObject>, SourceError> {
        let output = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.joined())
            .send()
            .await
            .map_err(map_sdk_error)?;

        if let Some(advertised) = output.content_length() {
            if advertised < 0 || advertised as u64 > self.limits.max_bytes {
                return Err(SourceError::TooLarge {
                    upstream_status: Some(200),
                });
            }
        }

        Ok(
            s3_identity(output.e_tag(), output.version_id()).map(|identity| IdentifiedObject {
                identity,
                upstream_status: Some(200),
            }),
        )
    }
}

#[async_trait]
impl Source for S3Source {
    async fn fetch(&self, key: &UpstreamKey) -> Result<FetchedObject, SourceError> {
        // One timeout for the whole exchange, including body streaming,
        // in addition to the SDK operation timeout configured above.
        match tokio::time::timeout(self.limits.timeout, self.fetch_inner(key)).await {
            Ok(result) => result,
            Err(_) => Err(SourceError::Timeout),
        }
    }

    async fn identify(&self, key: &UpstreamKey) -> Result<Option<IdentifiedObject>, SourceError> {
        match tokio::time::timeout(self.limits.timeout, self.identify_inner(key)).await {
            Ok(result) => result,
            Err(_) => Err(SourceError::Timeout),
        }
    }
}

fn s3_identity(e_tag: Option<&str>, version_id: Option<&str>) -> Option<ObjectIdentity> {
    let etag = strip_etag_quotes(e_tag?)?;
    let validator = match version_id {
        Some(version) if !version.is_empty() => format!("{etag};{version}"),
        _ => etag,
    };
    Some(ObjectIdentity::strong(validator))
}

fn strip_etag_quotes(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed);
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// Map SDK failures onto the shared taxonomy. A modeled missing-key error
/// or a raw upstream 404 is absence; access denial (403/AccessDenied) is
/// unavailability, never absence; dispatch/operation timeouts are timeouts.
fn map_sdk_error<E: ProvideErrorMetadata>(err: SdkError<E>) -> SourceError {
    match err {
        SdkError::ServiceError(context) => {
            let status = context.raw().status().as_u16();
            let code = context.err().code().unwrap_or("unknown");
            if status == 404 || code == "NoSuchKey" || code == "NotFound" {
                return SourceError::NotFound {
                    upstream_status: Some(404),
                };
            }
            // Everything else, including 403/AccessDenied, is
            // unavailability. Only the error code reaches the detail
            // string; response bodies and credentials never do.
            SourceError::Unavailable {
                upstream_status: Some(status),
                detail: format!("s3 service error: {code}"),
            }
        }
        SdkError::TimeoutError(_) => SourceError::Timeout,
        SdkError::DispatchFailure(failure) => {
            if failure.is_timeout() {
                SourceError::Timeout
            } else {
                SourceError::Unavailable {
                    upstream_status: None,
                    detail: "s3 dispatch failure".to_string(),
                }
            }
        }
        _ => SourceError::Unavailable {
            upstream_status: None,
            detail: "s3 request failed".to_string(),
        },
    }
}
