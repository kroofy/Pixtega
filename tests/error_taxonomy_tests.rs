//! Public error-string contract.
//!
//! The `public_message` strings are the exact `error` fields clients see in
//! JSON bodies; they are part of the public contract and must not drift.
//! `Display` must render the same stable message (it feeds logs and error
//! chains), and must never be empty.

use pixtega::errors::{ProcessError, RequestError, SourceError};

fn request_error_variants() -> Vec<RequestError> {
    vec![
        RequestError::TargetTooLong,
        RequestError::InvalidPrefix,
        RequestError::MissingMount,
        RequestError::UnknownMount,
        RequestError::MissingSourcePath,
        RequestError::InvalidSourcePath,
        RequestError::MissingTransform,
        RequestError::InvalidTransform,
        RequestError::DisallowedWidth,
        RequestError::UnconfiguredFormat,
        RequestError::DisallowedQuality,
        RequestError::QualityEqualsDefault,
        RequestError::InvalidQuery,
        RequestError::InvalidVersion,
    ]
}

fn source_error_variants() -> Vec<SourceError> {
    vec![
        SourceError::NotFound {
            upstream_status: Some(404),
        },
        SourceError::Unavailable {
            upstream_status: Some(502),
            detail: "internal detail".to_string(),
        },
        SourceError::TooLarge {
            upstream_status: None,
        },
        SourceError::Timeout,
    ]
}

fn process_error_variants() -> Vec<ProcessError> {
    vec![
        ProcessError::Undecodable {
            detail: "internal detail".to_string(),
        },
        ProcessError::TooManyPixels,
        ProcessError::Flatten {
            detail: "internal detail".to_string(),
        },
        ProcessError::Encode {
            detail: "internal detail".to_string(),
        },
    ]
}

#[test]
fn request_error_public_messages_are_the_documented_strings() {
    for (error, message) in [
        (RequestError::TargetTooLong, "request target too long"),
        (RequestError::InvalidPrefix, "unknown path prefix"),
        (RequestError::MissingMount, "missing mount"),
        (RequestError::UnknownMount, "unknown mount"),
        (RequestError::MissingSourcePath, "missing source path"),
        (RequestError::InvalidSourcePath, "invalid source path"),
        (RequestError::MissingTransform, "missing transform"),
        (RequestError::InvalidTransform, "invalid transform"),
        (RequestError::DisallowedWidth, "width not allowed"),
        (RequestError::UnconfiguredFormat, "format not configured"),
        (RequestError::DisallowedQuality, "quality not allowed"),
        (
            RequestError::QualityEqualsDefault,
            "quality equals the format default; omit q",
        ),
        (RequestError::InvalidQuery, "invalid query parameter"),
        (RequestError::InvalidVersion, "invalid version token"),
    ] {
        assert_eq!(error.public_message(), message, "{error:?}");
    }
}

#[test]
fn source_error_public_messages_are_the_documented_strings() {
    for (error, message) in [
        (
            SourceError::NotFound {
                upstream_status: None,
            },
            "source object not found",
        ),
        (
            SourceError::Unavailable {
                upstream_status: None,
                detail: String::new(),
            },
            "source unavailable",
        ),
        (
            SourceError::TooLarge {
                upstream_status: None,
            },
            "source object too large",
        ),
        (SourceError::Timeout, "source fetch timed out"),
    ] {
        assert_eq!(error.public_message(), message, "{error:?}");
    }
}

#[test]
fn process_error_public_messages_are_the_documented_strings() {
    for (error, message) in [
        (
            ProcessError::Undecodable {
                detail: String::new(),
            },
            "source is not a supported image",
        ),
        (
            ProcessError::TooManyPixels,
            "source image exceeds pixel limit",
        ),
        (
            ProcessError::Flatten {
                detail: String::new(),
            },
            "image flatten failed",
        ),
        (
            ProcessError::Encode {
                detail: String::new(),
            },
            "image encode failed",
        ),
    ] {
        assert_eq!(error.public_message(), message, "{error:?}");
    }
}

#[test]
fn source_error_detail_is_for_logs_only() {
    let err = SourceError::Unavailable {
        upstream_status: None,
        detail: "s3 dispatch failure".to_string(),
    };
    assert_eq!(err.detail(), Some("s3 dispatch failure"));
    assert_eq!(err.public_message(), "source unavailable");
    assert!(
        !err.public_message().contains("dispatch"),
        "public message must not carry detail"
    );

    let absent = SourceError::NotFound {
        upstream_status: Some(404),
    };
    assert_eq!(absent.detail(), None);
}

/// `Display` renders exactly the stable public message, never internal
/// detail (which could leak) and never an empty string.
#[test]
fn display_matches_public_message_and_never_leaks_detail() {
    for error in request_error_variants() {
        assert_eq!(error.to_string(), error.public_message(), "{error:?}");
        assert!(!error.to_string().is_empty(), "{error:?}");
    }
    for error in source_error_variants() {
        assert_eq!(error.to_string(), error.public_message(), "{error:?}");
        assert!(
            !error.to_string().contains("internal detail"),
            "{error:?} must not leak detail"
        );
    }
    for error in process_error_variants() {
        assert_eq!(error.to_string(), error.public_message(), "{error:?}");
        assert!(
            !error.to_string().contains("internal detail"),
            "{error:?} must not leak detail"
        );
    }
}
