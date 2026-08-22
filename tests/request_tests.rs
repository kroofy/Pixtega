//! Public request and policy tests.
//!
//! `parse_request` is a pure function: every Source in these configs points
//! at a nonexistent filesystem root, so any Source I/O during parsing would
//! fail loudly. Rejections here therefore happen without contacting any
//! Source, as the contract requires.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pixtega::config::{AppConfig, FilesystemSourceConfig, FormatPolicy, SourceConfig};
use pixtega::errors::RequestError;
use pixtega::request::{parse_request, MAX_TARGET_BYTES};
use pixtega::types::{OutputFormat, Transform};

const ALLOWED_WIDTHS: [u32; 4] = [320, 640, 1280, 1920];

/// RFC 3986 unreserved ASCII characters.
fn unreserved_chars() -> impl Iterator<Item = char> {
    (0u8..=0x7F)
        .filter(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        .map(|b| b as char)
}

fn fs_source(mount: &str, key_prefix: &[&str]) -> SourceConfig {
    SourceConfig::Filesystem(FilesystemSourceConfig {
        mount: mount.to_string(),
        key_prefix_segments: key_prefix.iter().map(|s| s.to_string()).collect(),
        // Deliberately nonexistent: request parsing must never touch it.
        root: PathBuf::from("/nonexistent/request-tests-never-read-this"),
    })
}

fn config_with_formats(formats: BTreeMap<OutputFormat, FormatPolicy>) -> AppConfig {
    AppConfig {
        listen_address: "127.0.0.1:8080".parse().unwrap(),
        path_prefix: "/images".to_string(),
        allowed_widths: ALLOWED_WIDTHS.to_vec(),
        max_download_bytes: 52_428_800,
        max_source_megapixels: 100,
        download_timeout_ms: 10_000,
        max_redirects: 3,
        max_concurrent_derivations: 8,
        unversioned_success_ttl_seconds: 3600,
        not_found_ttl_seconds: 60,
        formats,
        sources: vec![
            // Key Prefix differs from the Mount.
            fs_source("public", &["media"]),
            // Empty Key Prefix: requested path used unchanged.
            fs_source("fixtures", &[]),
            // Key Prefix equal to the Mount (the configured default).
            fs_source("archive", &["archive"]),
            // Multi-segment Key Prefix.
            fs_source("deep", &["originals", "large"]),
        ],
    }
}

/// webp: default 82, allows {60, 72, 90}; avif: default 55, allows {40, 65};
/// jpeg: default 85, empty quality allowlist.
fn base_config() -> AppConfig {
    let mut formats = BTreeMap::new();
    formats.insert(
        OutputFormat::Webp,
        FormatPolicy {
            default_quality: 82,
            allowed_qualities: vec![60, 72, 90],
        },
    );
    formats.insert(
        OutputFormat::Avif,
        FormatPolicy {
            default_quality: 55,
            allowed_qualities: vec![40, 65],
        },
    );
    formats.insert(
        OutputFormat::Jpeg,
        FormatPolicy {
            default_quality: 85,
            allowed_qualities: vec![],
        },
    );
    config_with_formats(formats)
}

/// Same as `base_config` but avif has no policy at all.
fn config_without_avif() -> AppConfig {
    let mut cfg = base_config();
    cfg.formats.remove(&OutputFormat::Avif);
    cfg
}

/// webp and avif both allow quality 72 (with different defaults).
fn config_with_shared_quality() -> AppConfig {
    let mut cfg = base_config();
    cfg.formats.insert(
        OutputFormat::Avif,
        FormatPolicy {
            default_quality: 55,
            allowed_qualities: vec![40, 65, 72],
        },
    );
    cfg
}

fn ok(cfg: &AppConfig, path: &str, query: Option<&str>) -> pixtega::types::ResolvedRequest {
    parse_request(cfg, path, query)
        .unwrap_or_else(|err| panic!("expected {path:?} (query {query:?}) to parse, got {err:?}"))
}

fn expect_err(cfg: &AppConfig, path: &str, query: Option<&str>, expected: RequestError) {
    assert_eq!(
        parse_request(cfg, path, query),
        Err(expected),
        "path {path:?}, query {query:?}"
    );
}

// --- A valid nested source path resolves to the expected Mount, upstream
// --- key, and Transform.

#[test]
fn valid_nested_path_resolves_mount_key_and_transform() {
    let cfg = base_config();
    let resolved = ok(
        &cfg,
        "/images/public/photos/2024/example.jpg/w640.webp",
        None,
    );
    assert_eq!(resolved.mount, "public");
    assert_eq!(
        resolved.upstream_key.joined(),
        "media/photos/2024/example.jpg"
    );
    assert_eq!(
        resolved.upstream_key.segments,
        vec!["media", "photos", "2024", "example.jpg"]
    );
    assert_eq!(
        resolved.transform,
        Transform {
            width: 640,
            format: OutputFormat::Webp,
            quality: 82, // policy default; q omitted
        }
    );
    assert!(!resolved.versioned);
}

#[test]
fn every_allowed_width_and_configured_format_resolves() {
    let cfg = base_config();
    let cases = [
        (OutputFormat::Webp, 82u32),
        (OutputFormat::Avif, 55),
        (OutputFormat::Jpeg, 85),
    ];
    for width in ALLOWED_WIDTHS {
        for (format, default_quality) in cases {
            let path = format!("/images/fixtures/pic.png/w{width}.{format}");
            let resolved = ok(&cfg, &path, None);
            assert_eq!(
                resolved.transform,
                Transform {
                    width,
                    format,
                    quality: default_quality,
                }
            );
        }
    }
}

#[test]
fn explicit_allowed_quality_is_resolved() {
    let cfg = base_config();
    for (transform_seg, width, format, quality) in [
        ("w640,q60.webp", 640, OutputFormat::Webp, 60),
        ("w640,q72.webp", 640, OutputFormat::Webp, 72),
        ("w1920,q90.webp", 1920, OutputFormat::Webp, 90),
        ("w320,q40.avif", 320, OutputFormat::Avif, 40),
        ("w1280,q65.avif", 1280, OutputFormat::Avif, 65),
    ] {
        let path = format!("/images/public/a.jpg/{transform_seg}");
        let resolved = ok(&cfg, &path, None);
        assert_eq!(
            resolved.transform,
            Transform {
                width,
                format,
                quality,
            }
        );
    }
}

// --- A Key Prefix may differ from the Mount or be empty.

#[test]
fn key_prefix_may_differ_from_mount_or_be_empty() {
    let cfg = base_config();
    for (path, mount, key) in [
        (
            "/images/public/photos/a.jpg/w640.webp",
            "public",
            "media/photos/a.jpg",
        ),
        (
            "/images/fixtures/photos/a.jpg/w640.webp",
            "fixtures",
            "photos/a.jpg",
        ),
        (
            "/images/archive/photos/a.jpg/w640.webp",
            "archive",
            "archive/photos/a.jpg",
        ),
        (
            "/images/deep/photos/a.jpg/w640.webp",
            "deep",
            "originals/large/photos/a.jpg",
        ),
    ] {
        let resolved = ok(&cfg, path, None);
        assert_eq!(resolved.mount, mount, "path {path:?}");
        assert_eq!(resolved.upstream_key.joined(), key, "path {path:?}");
    }
}

// --- Dots in filenames and directories survive unchanged. The validated
// --- path preserves dots, underscores, Unicode, and nesting.

#[test]
fn dots_underscores_unicode_and_nesting_survive() {
    let cfg = base_config();
    for (path, expected_key) in [
        (
            "/images/fixtures/release.2024/img.v2.final.jpg/w640.webp",
            "release.2024/img.v2.final.jpg",
        ),
        (
            "/images/fixtures/dir_name/file_name.ext/w640.webp",
            "dir_name/file_name.ext",
        ),
        ("/images/fixtures/a.b.c.d/w640.webp", "a.b.c.d"),
        ("/images/fixtures/...hidden/w640.webp", "...hidden"),
        ("/images/fixtures/..a/b../w640.webp", "..a/b.."),
        ("/images/fixtures/~tilde/w640.webp", "~tilde"),
        // Percent-encoded UTF-8 decodes exactly once.
        ("/images/fixtures/caf%C3%A9.jpg/w640.webp", "café.jpg"),
        (
            "/images/fixtures/%E5%86%99%E7%9C%9F/x.png/w640.webp",
            "写真/x.png",
        ),
        ("/images/fixtures/a%20b.jpg/w640.webp", "a b.jpg"),
        // %25 decodes to a literal percent; a second decode that creates no
        // traversal or delimiter is acceptable.
        ("/images/fixtures/a%2520b/w640.webp", "a%20b"),
        ("/images/fixtures/100%25.jpg/w640.webp", "100%.jpg"),
        (
            "/images/fixtures/a/b/c/d/e/f.jpg/w640.webp",
            "a/b/c/d/e/f.jpg",
        ),
    ] {
        let resolved = ok(&cfg, path, None);
        assert_eq!(
            resolved.upstream_key.joined(),
            expected_key,
            "path {path:?}"
        );
    }
}

// --- Missing `v` is accepted.

#[test]
fn missing_v_is_accepted() {
    let cfg = base_config();
    for query in [None, Some("")] {
        let resolved = ok(&cfg, "/images/public/a.jpg/w640.webp", query);
        assert!(!resolved.versioned, "query {query:?}");
    }
}

#[test]
fn valid_v_marks_the_request_versioned() {
    let cfg = base_config();
    for v in [
        "7d91c2",
        "a",
        "A",
        "0",
        "-",
        ".",
        "_",
        "~",
        "a.B_c~d-9",
        &"x".repeat(128),
    ] {
        let query = format!("v={v}");
        let resolved = ok(&cfg, "/images/public/a.jpg/w640.webp", Some(&query));
        assert!(resolved.versioned, "query {query:?}");
    }
}

#[test]
fn generated_valid_v_tokens_are_accepted() {
    let cfg = base_config();
    let alphabet: Vec<char> = unreserved_chars().collect();
    // Deterministic linear congruential generator; no external crates.
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };
    for _ in 0..64 {
        let len = (next() % 128) as usize + 1;
        let token: String = (0..len)
            .map(|_| alphabet[(next() as usize) % alphabet.len()])
            .collect();
        let query = format!("v={token}");
        let resolved = ok(&cfg, "/images/public/a.jpg/w640.webp", Some(&query));
        assert!(resolved.versioned, "query {query:?}");
    }
}

// --- Empty, overlong, percent-encoded, or non-canonical `v` values return
// --- 400 (InvalidVersion).

#[test]
fn invalid_v_values_are_rejected() {
    let cfg = base_config();
    let overlong = "x".repeat(129);
    let queries: Vec<String> = vec![
        "v=".to_string(),          // empty
        "v".to_string(),           // present but valueless
        format!("v={overlong}"),   // overlong (129)
        "v=%41".to_string(),       // percent-encoded (would be 'A')
        "v=abc%2E".to_string(),    // percent-encoded dot
        "v=a%".to_string(),        // stray percent
        "v=100%".to_string(),      // trailing percent
        "v=a b".to_string(),       // space
        "v=a+b".to_string(),       // '+' is not unreserved
        "v=a/b".to_string(),       // slash
        "v=a\\b".to_string(),      // backslash
        "v=a=b".to_string(),       // '='
        "v=a!".to_string(),        // '!'
        "v=caf\u{e9}".to_string(), // non-ASCII
        "v=\u{7}".to_string(),     // control character
    ];
    for query in &queries {
        expect_err(
            &cfg,
            "/images/public/a.jpg/w640.webp",
            Some(query),
            RequestError::InvalidVersion,
        );
    }
}

// --- Repeated `v` and unknown query parameters return 400 (InvalidQuery).

#[test]
fn repeated_v_and_unknown_query_parameters_are_rejected() {
    let cfg = base_config();
    for query in [
        "v=1&v=2",
        "v=1&v=1",
        "foo=1",
        "foo",
        "=x",
        "=",
        "V=1", // case-sensitive
        "vv=1",
        "v =1",
        "%76=1", // encoded 'v' key is not the literal key "v"
        "v=1&foo=2",
        "foo=1&v=1",
        "v=1&",
        "&v=1",
        "&",
        "w=640",
    ] {
        expect_err(
            &cfg,
            "/images/public/a.jpg/w640.webp",
            Some(query),
            RequestError::InvalidQuery,
        );
    }
}

// --- Non-canonical percent encoding and a request target over 8192 bytes
// --- return 400.

#[test]
fn percent_encoded_unreserved_ascii_is_rejected_generated() {
    let cfg = base_config();
    // Every unreserved ASCII character has exactly one spelling: itself.
    for c in unreserved_chars() {
        let triplet = format!("%{:02X}", c as u32);
        let path = format!("/images/public/a{triplet}b/w640.webp");
        expect_err(&cfg, &path, None, RequestError::InvalidSourcePath);
    }
}

#[test]
fn lowercase_and_invalid_percent_triplets_are_rejected() {
    let cfg = base_config();
    for segment in [
        "%2f",
        "%5c",
        "%2e",
        "%c3%a9",
        "%C3%a9",
        "%e2%82%ac",
        "a%2db", // lowercase hex
        "%",
        "%2",
        "a%",
        "ab%C",
        "%G1",
        "%2G",
        "%ZZ",
        "% 20", // invalid triplets
        "%FF",
        "%C3%C3",
        "%E0%A0", // decodes to invalid UTF-8
    ] {
        let path = format!("/images/public/{segment}/w640.webp");
        expect_err(&cfg, &path, None, RequestError::InvalidSourcePath);
    }
}

#[test]
fn literal_bytes_outside_the_wire_alphabet_are_rejected() {
    let cfg = base_config();
    for bad in [
        ' ', '(', ')', '&', ':', '+', '@', '=', '!', '*', '\'', ',', ';', '$', 'é',
    ] {
        let path = format!("/images/public/a{bad}b/w640.webp");
        expect_err(&cfg, &path, None, RequestError::InvalidSourcePath);
    }
}

#[test]
fn target_over_8192_bytes_is_rejected() {
    let cfg = base_config();

    // Path alone over the limit.
    let long = format!("/images/public/{}/w640.webp", "a".repeat(MAX_TARGET_BYTES));
    expect_err(&cfg, &long, None, RequestError::TargetTooLong);

    // A valid request padded to exactly the limit is accepted; one more
    // byte is not. Query bytes and the '?' separator count.
    let suffix = "/w640.webp";
    let query = "v=7d91c2";
    let head = "/images/public/";
    let pad = MAX_TARGET_BYTES - head.len() - suffix.len() - 1 - query.len();
    let path = format!("{head}{}{suffix}", "a".repeat(pad));
    assert_eq!(path.len() + 1 + query.len(), MAX_TARGET_BYTES);
    let resolved = ok(&cfg, &path, Some(query));
    assert!(resolved.versioned);

    let path_plus_one = format!("{head}{}{suffix}", "a".repeat(pad + 1));
    expect_err(
        &cfg,
        &path_plus_one,
        Some(query),
        RequestError::TargetTooLong,
    );

    // The length check applies before any other validation.
    let junk = format!("/elsewhere/{}", "%".repeat(MAX_TARGET_BYTES));
    expect_err(&cfg, &junk, None, RequestError::TargetTooLong);
}

// --- Unknown Mounts return 400 without contacting any Source. (All
// --- configured roots are nonexistent, so parsing cannot perform I/O.)

#[test]
fn unknown_mounts_are_rejected() {
    let cfg = base_config();
    for mount in [
        "nope",
        "Public", // mounts are case-sensitive
        "publics",
        "pub%6Cic", // percent-encoded mount does not match literally
        "public%20",
        "media", // a Key Prefix is not a Mount
    ] {
        let path = format!("/images/{mount}/a.jpg/w640.webp");
        expect_err(&cfg, &path, None, RequestError::UnknownMount);
    }
}

#[test]
fn missing_mount_and_wrong_prefix_are_rejected() {
    let cfg = base_config();
    expect_err(&cfg, "/images/", None, RequestError::MissingMount);
    expect_err(
        &cfg,
        "/images//a.jpg/w640.webp",
        None,
        RequestError::MissingMount,
    );
    for path in [
        "/",
        "/img/public/a.jpg/w640.webp",
        "/imagesx/public/a.jpg/w640.webp",
        "/images", // prefix must be followed by '/'
        "images/public/a.jpg/w640.webp",
        "/IMAGES/public/a.jpg/w640.webp",
        "//images/public/a.jpg/w640.webp",
    ] {
        expect_err(&cfg, path, None, RequestError::InvalidPrefix);
    }
}

// --- Widths, unconfigured formats, and qualities outside the selected
// --- format policy return 400 without a fetch.

#[test]
fn widths_outside_the_allowlist_are_rejected() {
    let cfg = base_config();
    for width in ["0", "1", "2", "100", "321", "639", "641", "1000", "16384"] {
        let path = format!("/images/public/a.jpg/w{width}.webp");
        expect_err(&cfg, &path, None, RequestError::DisallowedWidth);
    }
    // Grammatically canonical but larger than any configurable width,
    // including values that overflow machine integers.
    for width in ["16385", "99999", "4294967296", "18446744073709551616"] {
        let path = format!("/images/public/a.jpg/w{width}.webp");
        expect_err(&cfg, &path, None, RequestError::DisallowedWidth);
    }
}

#[test]
fn unconfigured_formats_are_rejected() {
    let cfg = config_without_avif();
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640.avif",
        None,
        RequestError::UnconfiguredFormat,
    );
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640,q40.avif",
        None,
        RequestError::UnconfiguredFormat,
    );
    // The other formats keep working.
    ok(&cfg, "/images/public/a.jpg/w640.webp", None);
    ok(&cfg, "/images/public/a.jpg/w640.jpeg", None);
}

#[test]
fn qualities_outside_the_selected_policy_are_rejected() {
    let cfg = base_config();
    for q in ["1", "59", "61", "71", "73", "89", "91", "100"] {
        let path = format!("/images/public/a.jpg/w640,q{q}.webp");
        expect_err(&cfg, &path, None, RequestError::DisallowedQuality);
    }
    // Canonical decimals above 100 or beyond u64 can never be allowed.
    for q in ["101", "1000", "18446744073709551616"] {
        let path = format!("/images/public/a.jpg/w640,q{q}.webp");
        expect_err(&cfg, &path, None, RequestError::DisallowedQuality);
    }
}

// --- Quality is closed independently for each format whose allowlist is
// --- empty.

#[test]
fn empty_quality_allowlist_permits_no_explicit_quality() {
    let cfg = base_config(); // jpeg: default 85, empty allowlist
    for q in [1u32, 50, 60, 72, 84, 86, 90, 92, 100] {
        let path = format!("/images/public/a.jpg/w640,q{q}.jpeg");
        expect_err(&cfg, &path, None, RequestError::DisallowedQuality);
    }
    // The default spelled explicitly is rejected as non-canonical, not
    // silently allowed through the empty allowlist.
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640,q85.jpeg",
        None,
        RequestError::QualityEqualsDefault,
    );
    // Omitting q still resolves the configured default.
    let resolved = ok(&cfg, "/images/public/a.jpg/w640.jpeg", None);
    assert_eq!(resolved.transform.quality, 85);
}

// --- A quality allowed for one format remains invalid for another unless
// --- both policies list it.

#[test]
fn quality_allowlists_are_per_format() {
    let cfg = base_config();
    // 60 is allowed for webp only.
    ok(&cfg, "/images/public/a.jpg/w640,q60.webp", None);
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640,q60.avif",
        None,
        RequestError::DisallowedQuality,
    );
    // 40 is allowed for avif only.
    ok(&cfg, "/images/public/a.jpg/w640,q40.avif", None);
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640,q40.webp",
        None,
        RequestError::DisallowedQuality,
    );
    // When both policies list a quality, both accept it.
    let shared = config_with_shared_quality();
    assert_eq!(
        ok(&shared, "/images/public/a.jpg/w640,q72.webp", None)
            .transform
            .quality,
        72
    );
    assert_eq!(
        ok(&shared, "/images/public/a.jpg/w640,q72.avif", None)
            .transform
            .quality,
        72
    );
    // ...but the base config's avif still rejects it.
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640,q72.avif",
        None,
        RequestError::DisallowedQuality,
    );
}

// --- Quality equal to a format default is rejected.

#[test]
fn quality_equal_to_the_format_default_is_rejected() {
    let cfg = base_config();
    for (q, format) in [(82, "webp"), (55, "avif"), (85, "jpeg")] {
        let path = format!("/images/public/a.jpg/w640,q{q}.{format}");
        expect_err(&cfg, &path, None, RequestError::QualityEqualsDefault);
    }
    // Omitting q is the canonical spelling and resolves the default.
    for (format, default_quality) in [
        (OutputFormat::Webp, 82),
        (OutputFormat::Avif, 55),
        (OutputFormat::Jpeg, 85),
    ] {
        let path = format!("/images/public/a.jpg/w640.{format}");
        assert_eq!(ok(&cfg, &path, None).transform.quality, default_quality);
    }
}

// --- Field reordering, repetition, aliases, signs, and leading zeros are
// --- rejected.

#[test]
fn transform_grammar_violations_are_rejected() {
    let cfg = base_config();
    for transform in [
        // reordering
        "q60,w640.webp",
        "640w.webp",
        "webp.w640",
        // repetition
        "w640,w640.webp",
        "w640,q60,q60.webp",
        "w640,q60,w640.webp",
        // aliases and bad formats
        "w640.jpg",
        "w640.JPEG",
        "w640.WEBP",
        "w640.Webp",
        "w640.png",
        "w640.webp2",
        "w640.jpeg ",
        // signs
        "w+640.webp",
        "w-640.webp",
        "w640,q+60.webp",
        "w640,q-60.webp",
        // leading zeros
        "w0640.webp",
        "w00.webp",
        "w640,q060.webp",
        "w640,q00.webp",
        // missing / empty fields
        "w640",
        "w640,q60",
        ".webp",
        "w.webp",
        "w640,.webp",
        "w640,q.webp",
        ",q60.webp",
        "w640,q60,.webp",
        "",
        // extra dots
        "w640..webp",
        "w640.webp.webp",
        "w640.q60.webp",
        "w.640.webp",
        // non-decimal payloads and stray characters
        "wabc.webp",
        "w6a0.webp",
        "w640,qx.webp",
        "w 640.webp",
        "w640 .webp",
        "w640,q60 .webp",
        "x640.webp",
        "W640.webp",
        "w640,Q60.webp",
        // percent encoding never satisfies the grammar
        "w640%2Ewebp",
        "%77640.webp",
        "w640.web%70",
        "w640\u{0}.webp",
    ] {
        let path = format!("/images/public/a.jpg/{transform}");
        expect_err(&cfg, &path, None, RequestError::InvalidTransform);
    }
    // Trailing slash leaves an empty transform segment.
    expect_err(
        &cfg,
        "/images/public/a.jpg/w640.webp/",
        None,
        RequestError::InvalidTransform,
    );
    // Width 0 is grammatically canonical; it fails the allowlist instead.
    expect_err(
        &cfg,
        "/images/public/a.jpg/w0.webp",
        None,
        RequestError::DisallowedWidth,
    );
}

// --- Missing source path or Transform is rejected.

#[test]
fn missing_source_path_or_transform_is_rejected() {
    let cfg = base_config();
    // Mount only: nothing between the Mount and where a Transform would be.
    expect_err(
        &cfg,
        "/images/public",
        None,
        RequestError::MissingSourcePath,
    );
    // Mount plus one segment: the trailing Transform is missing.
    expect_err(
        &cfg,
        "/images/public/a.jpg",
        None,
        RequestError::MissingTransform,
    );
    expect_err(
        &cfg,
        "/images/public/w640.webp",
        None,
        RequestError::MissingTransform,
    );
}

// --- Literal and encoded traversal attempts are rejected.

#[test]
fn literal_and_encoded_traversal_is_rejected() {
    let cfg = base_config();
    for segment in [
        // literal dot segments
        ".",
        "..",
        // singly percent-encoded dots are also non-canonical encodings
        "%2E",
        "%2E%2E",
        "%2E.",
        ".%2E",
        "%2e%2e", // lowercase is doubly invalid
        // encoded path delimiters
        "a%2Fb",
        "a%5Cb",
        "%2F",
        "%5C",
        "..%2F..",
        // literal backslash
        "a\\b",
        "..\\..",
        // double-encoded traversal and delimiters
        "%252E%252E",
        "%252e%252e",
        "%252E",
        "%252F",
        "%255C",
        "a%252Fb",
        "a%255Cb",
        // NUL and control characters, raw and encoded
        "%00",
        "a%00b",
        "%1F",
        "%0A",
        "%7F",
        "a\u{7}b",
        "a\tb",
    ] {
        for path in [
            format!("/images/public/{segment}/w640.webp"),
            format!("/images/public/{segment}/a.jpg/w640.webp"),
            format!("/images/public/a/{segment}/b.jpg/w640.webp"),
        ] {
            expect_err(&cfg, &path, None, RequestError::InvalidSourcePath);
        }
    }
    // Empty interior segments are rejected too.
    expect_err(
        &cfg,
        "/images/public//a.jpg/w640.webp",
        None,
        RequestError::InvalidSourcePath,
    );
    expect_err(
        &cfg,
        "/images/public/a//b.jpg/w640.webp",
        None,
        RequestError::InvalidSourcePath,
    );
    // Benign double-percent content that creates no traversal on a second
    // decode stays accepted.
    assert_eq!(
        ok(&cfg, "/images/fixtures/a%2520b/w640.webp", None)
            .upstream_key
            .joined(),
        "a%20b"
    );
}

// --- The versioned flag composes with everything else.

#[test]
fn versioned_and_unversioned_requests_resolve_identically_except_the_flag() {
    let cfg = base_config();
    let unversioned = ok(&cfg, "/images/public/a.jpg/w640,q60.webp", None);
    let versioned = ok(&cfg, "/images/public/a.jpg/w640,q60.webp", Some("v=7d91c2"));
    assert!(!unversioned.versioned);
    assert!(versioned.versioned);
    assert_eq!(unversioned.mount, versioned.mount);
    assert_eq!(unversioned.upstream_key, versioned.upstream_key);
    assert_eq!(unversioned.transform, versioned.transform);
}
