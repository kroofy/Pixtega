//! Behavior tests for configuration loading and validation.
//!
//! Every test drives the public `load_from_str` /
//! `load_from_file` interface and asserts only on Ok/Err plus loose message
//! substrings (the offending field or mount name).

use std::path::Path;

use pixtega::config::{load_from_file, load_from_str, AppConfig, SourceConfig};
use pixtega::errors::ConfigError;
use pixtega::types::OutputFormat;

// ---------------------------------------------------------------------------
// Helpers: a known-good configuration with replaceable parts.
// ---------------------------------------------------------------------------

const TOP_FIELDS: &[(&str, &str)] = &[
    ("listen_address", "\"127.0.0.1:8080\""),
    ("path_prefix", "\"/images\""),
    ("allowed_widths", "[320, 640]"),
    ("max_download_bytes", "1000000"),
    ("max_source_megapixels", "10"),
    ("download_timeout_ms", "1000"),
    ("max_redirects", "2"),
    ("max_concurrent_derivations", "4"),
    ("unversioned_success_ttl_seconds", "60"),
    ("not_found_ttl_seconds", "30"),
];

const FORMATS: &str = "[formats.webp]\ndefault_quality = 80\nallowed_qualities = [60, 90]\n";

const SOURCES: &str =
    "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"https://images.example.test\"\n";

/// Build a configuration. Each override replaces a top-level field's value;
/// the special value `<omit>` drops the field entirely.
fn config_with(overrides: &[(&str, &str)], formats: &str, sources: &str) -> String {
    let mut out = String::new();
    for (key, default_value) in TOP_FIELDS {
        let value = overrides
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
            .unwrap_or(default_value);
        if value != "<omit>" {
            out.push_str(&format!("{key} = {value}\n"));
        }
    }
    out.push('\n');
    out.push_str(formats);
    out.push('\n');
    out.push_str(sources);
    out
}

fn valid_config() -> String {
    config_with(&[], FORMATS, SOURCES)
}

fn load(toml_text: &str) -> Result<AppConfig, ConfigError> {
    load_from_str(toml_text, Path::new("."))
}

#[track_caller]
fn assert_rejected(toml_text: &str, needle: &str) {
    let err = load(toml_text).expect_err(&format!(
        "configuration should be rejected (expected message mentioning {needle:?}):\n{toml_text}"
    ));
    let message = err.to_string();
    assert!(
        message.contains(needle),
        "error message {message:?} should mention {needle:?}"
    );
}

fn http_source(extra: &str) -> String {
    format!(
        "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"https://images.example.test\"\n{extra}"
    )
}

/// A filesystem source whose root is `.` so it always resolves during tests.
fn filesystem_source(extra: &str) -> String {
    format!("[[sources]]\nmount = \"local\"\ntransport = \"filesystem\"\nroot = \".\"\n{extra}")
}

fn s3_source(extra: &str) -> String {
    format!(
        "[[sources]]\nmount = \"bucketed\"\ntransport = \"s3\"\nbucket = \"b\"\nregion = \"us-east-1\"\n{extra}"
    )
}

fn only_source(config: &AppConfig) -> &SourceConfig {
    assert_eq!(config.sources.len(), 1);
    &config.sources[0]
}

// ---------------------------------------------------------------------------
// Valid configurations.
// ---------------------------------------------------------------------------

#[test]
fn valid_configuration_parses() {
    let config = load(&valid_config()).expect("baseline configuration must parse");
    assert_eq!(config.path_prefix, "/images");
    assert_eq!(config.allowed_widths, vec![320, 640]);
    assert_eq!(config.max_download_bytes, 1_000_000);
    assert_eq!(config.max_redirects, 2);
    assert_eq!(config.max_concurrent_derivations, 4);
    let policy = config
        .format_policy(OutputFormat::Webp)
        .expect("webp policy");
    assert_eq!(policy.default_quality, 80);
    assert_eq!(policy.allowed_qualities, vec![60, 90]);
    match only_source(&config) {
        SourceConfig::Http(http) => {
            assert_eq!(http.mount, "pics");
            assert_eq!(http.base_url.as_str(), "https://images.example.test/");
            assert!(http.ca_certificate_file.is_none());
        }
        other => panic!("expected an http source, got {other:?}"),
    }
}

#[test]
fn example_configuration_parses_via_load_from_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    let config = load_from_file(&path).expect("config.example.toml must parse");

    assert_eq!(config.listen_address.port(), 8080);
    assert_eq!(config.path_prefix, "/images");
    assert_eq!(config.allowed_widths, vec![320, 640, 1280, 1920]);
    assert_eq!(config.sources.len(), 3);

    let webp = config.format_policy(OutputFormat::Webp).expect("webp");
    assert_eq!(webp.default_quality, 82);
    assert_eq!(webp.allowed_qualities, vec![60, 72, 90]);
    let avif = config.format_policy(OutputFormat::Avif).expect("avif");
    assert_eq!(avif.default_quality, 55);
    assert_eq!(avif.allowed_qualities, vec![40, 65]);
    let jpeg = config.format_policy(OutputFormat::Jpeg).expect("jpeg");
    assert_eq!(jpeg.default_quality, 85);
    assert_eq!(jpeg.allowed_qualities, vec![70, 92]);

    let public = config.source_for_mount("public").expect("public source");
    assert_eq!(public.key_prefix_segments(), ["media"]);

    let fixtures = config
        .source_for_mount("fixtures")
        .expect("fixtures source");
    assert!(fixtures.key_prefix_segments().is_empty());
    match fixtures {
        SourceConfig::Filesystem(fs) => {
            assert!(fs.root.is_absolute(), "root must be canonicalized");
            assert!(fs.root.is_dir(), "root must be a directory");
        }
        other => panic!("fixtures should be a filesystem source, got {other:?}"),
    }

    let archive = config.source_for_mount("archive").expect("archive source");
    assert_eq!(archive.key_prefix_segments(), ["originals"]);
    match archive {
        SourceConfig::S3(s3) => {
            assert_eq!(s3.bucket, "example-image-bucket");
            assert_eq!(s3.region, "us-east-1");
            assert!(s3.endpoint_url.is_none());
            assert!(!s3.force_path_style);
        }
        other => panic!("archive should be an s3 source, got {other:?}"),
    }
}

#[test]
fn path_prefix_defaults_to_images_when_omitted() {
    let toml = config_with(&[("path_prefix", "<omit>")], FORMATS, SOURCES);
    let config = load(&toml).expect("omitted path_prefix must default");
    assert_eq!(config.path_prefix, "/images");
}

#[test]
fn custom_multi_segment_path_prefix_is_accepted() {
    let toml = config_with(&[("path_prefix", "\"/img/v2\"")], FORMATS, SOURCES);
    let config = load(&toml).expect("multi-segment prefix must parse");
    assert_eq!(config.path_prefix, "/img/v2");
}

#[test]
fn boundary_numeric_values_are_accepted() {
    let boundaries: &[(&str, &[&str])] = &[
        ("allowed_widths", &["[1, 16384]"]),
        ("max_download_bytes", &["1", "104857600"]),
        ("max_source_megapixels", &["1", "500"]),
        ("download_timeout_ms", &["1", "60000"]),
        ("max_redirects", &["0", "10"]),
        ("max_concurrent_derivations", &["1", "64"]),
        ("unversioned_success_ttl_seconds", &["1", "86400"]),
        ("not_found_ttl_seconds", &["1", "3600"]),
    ];
    for (field, values) in boundaries {
        for value in *values {
            let toml = config_with(&[(field, value)], FORMATS, SOURCES);
            load(&toml).unwrap_or_else(|e| panic!("{field} = {value} should be valid: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level field validation.
// ---------------------------------------------------------------------------

#[test]
fn missing_required_top_level_fields_are_rejected() {
    // Every top-level field except path_prefix is required.
    for (field, _) in TOP_FIELDS.iter().filter(|(f, _)| *f != "path_prefix") {
        let toml = config_with(&[(field, "<omit>")], FORMATS, SOURCES);
        assert_rejected(&toml, field);
    }
    assert_rejected(&config_with(&[], "", SOURCES), "formats");
    assert_rejected(&config_with(&[], FORMATS, ""), "sources");
}

#[test]
fn invalid_listen_address_is_rejected() {
    for value in ["\"not-an-address\"", "\"127.0.0.1\"", "\"\""] {
        let toml = config_with(&[("listen_address", value)], FORMATS, SOURCES);
        assert_rejected(&toml, "listen_address");
    }
}

#[test]
fn invalid_path_prefix_is_rejected() {
    let invalid = [
        "\"images\"",   // no leading slash
        "\"/images/\"", // trailing slash
        "\"/\"",        // no segments
        "\"//img\"",    // empty segment
        "\"/img//x\"",  // empty inner segment
        "\"/./img\"",   // dot segment
        "\"/img/..\"",  // dot-dot segment
        "\"\"",         // empty
    ];
    for value in invalid {
        let toml = config_with(&[("path_prefix", value)], FORMATS, SOURCES);
        assert_rejected(&toml, "path_prefix");
    }
}

#[test]
fn invalid_width_allowlists_are_rejected() {
    let invalid = [
        "[]",         // empty allowlist
        "[0]",        // width 0
        "[0, 320]",   // width 0 among valid widths
        "[16385]",    // above range
        "[-5]",       // below range
        "[320, 320]", // duplicate
        "[320, 640, 320]",
    ];
    for value in invalid {
        let toml = config_with(&[("allowed_widths", value)], FORMATS, SOURCES);
        assert_rejected(&toml, "width");
    }
}

#[test]
fn out_of_range_limits_timeouts_and_ttls_are_rejected() {
    let cases: &[(&str, &[&str])] = &[
        ("max_download_bytes", &["0", "-1", "104857601"]),
        ("max_source_megapixels", &["0", "501"]),
        ("download_timeout_ms", &["0", "60001"]),
        ("max_redirects", &["-1", "11"]),
        ("max_concurrent_derivations", &["0", "65"]),
        ("unversioned_success_ttl_seconds", &["0", "86401"]),
        ("not_found_ttl_seconds", &["0", "3601"]),
    ];
    for (field, values) in cases {
        for value in *values {
            let toml = config_with(&[(field, value)], FORMATS, SOURCES);
            assert_rejected(&toml, field);
        }
    }
}

#[test]
fn unknown_top_level_fields_are_rejected() {
    let toml = format!("bogus_setting = 1\n{}", valid_config());
    assert_rejected(&toml, "bogus_setting");
}

// ---------------------------------------------------------------------------
// Format policies.
// ---------------------------------------------------------------------------

#[test]
fn empty_format_table_is_rejected() {
    let toml = config_with(&[], "formats = {}\n", SOURCES);
    assert_rejected(&toml, "format");
}

#[test]
fn unknown_format_policy_is_rejected() {
    for name in ["png", "jpg", "gif", "WEBP"] {
        let formats = format!("[formats.{name}]\ndefault_quality = 80\n");
        let toml = config_with(&[], &formats, SOURCES);
        assert_rejected(&toml, name);
    }
}

#[test]
fn duplicate_format_policy_is_rejected() {
    let formats = "[formats.webp]\ndefault_quality = 80\n\n[formats.webp]\ndefault_quality = 70\n";
    let toml = config_with(&[], formats, SOURCES);
    assert_rejected(&toml, "webp");
}

#[test]
fn missing_default_quality_is_rejected() {
    let toml = config_with(&[], "[formats.webp]\nallowed_qualities = [60]\n", SOURCES);
    assert_rejected(&toml, "default_quality");
}

#[test]
fn out_of_range_qualities_are_rejected() {
    for default in ["0", "101", "-3"] {
        let formats = format!("[formats.webp]\ndefault_quality = {default}\n");
        assert_rejected(&config_with(&[], &formats, SOURCES), "default_quality");
    }
    for allowed in ["[0]", "[101]", "[60, 0]"] {
        let formats =
            format!("[formats.webp]\ndefault_quality = 80\nallowed_qualities = {allowed}\n");
        assert_rejected(&config_with(&[], &formats, SOURCES), "allowed_qualities");
    }
}

#[test]
fn duplicate_allowed_qualities_are_rejected() {
    let formats = "[formats.webp]\ndefault_quality = 80\nallowed_qualities = [60, 60]\n";
    assert_rejected(&config_with(&[], formats, SOURCES), "allowed_qualities");
}

#[test]
fn allowed_quality_equal_to_default_is_rejected() {
    let formats = "[formats.webp]\ndefault_quality = 80\nallowed_qualities = [60, 80]\n";
    assert_rejected(&config_with(&[], formats, SOURCES), "default");
}

#[test]
fn missing_allowed_qualities_defaults_to_empty_for_that_format_only() {
    let formats = "[formats.webp]\ndefault_quality = 80\n\n\
                   [formats.jpeg]\ndefault_quality = 85\nallowed_qualities = [70]\n";
    let config = load(&config_with(&[], formats, SOURCES)).expect("must parse");
    // No explicit quality is possible for webp.
    let webp = config.format_policy(OutputFormat::Webp).expect("webp");
    assert!(webp.allowed_qualities.is_empty());
    // ...while jpeg still has its own allowlist.
    let jpeg = config.format_policy(OutputFormat::Jpeg).expect("jpeg");
    assert_eq!(jpeg.allowed_qualities, vec![70]);
}

#[test]
fn formats_may_configure_different_defaults_and_allowlists() {
    let formats = "[formats.webp]\ndefault_quality = 80\nallowed_qualities = [60]\n\n\
                   [formats.avif]\ndefault_quality = 50\n\n\
                   [formats.jpeg]\ndefault_quality = 85\nallowed_qualities = [70, 92]\n";
    let config = load(&config_with(&[], formats, SOURCES)).expect("must parse");
    let webp = config.format_policy(OutputFormat::Webp).expect("webp");
    let avif = config.format_policy(OutputFormat::Avif).expect("avif");
    let jpeg = config.format_policy(OutputFormat::Jpeg).expect("jpeg");
    assert_eq!(webp.default_quality, 80);
    assert_eq!(avif.default_quality, 50);
    assert_eq!(jpeg.default_quality, 85);
    assert_eq!(webp.allowed_qualities, vec![60]);
    assert!(avif.allowed_qualities.is_empty());
    assert_eq!(jpeg.allowed_qualities, vec![70, 92]);
}

#[test]
fn unknown_format_policy_fields_are_rejected() {
    let formats = "[formats.webp]\ndefault_quality = 80\ncompression_effort = 9\n";
    assert_rejected(&config_with(&[], formats, SOURCES), "compression_effort");
}

// ---------------------------------------------------------------------------
// Sources: mounts, transports, mutual exclusion.
// ---------------------------------------------------------------------------

#[test]
fn empty_source_list_is_rejected() {
    let mut toml = config_with(&[], "", "");
    toml.push_str("sources = []\n\n");
    toml.push_str(FORMATS);
    assert_rejected(&toml, "source");
}

#[test]
fn invalid_mount_names_are_rejected() {
    let long_mount = "a".repeat(33);
    let invalid = [
        "",       // empty
        "Public", // uppercase
        "1pic",   // starts with a digit
        "-abc",   // starts with a hyphen
        "a_b",    // underscore
        "a.b",    // dot
        "a b",    // space
        long_mount.as_str(),
    ];
    for mount in invalid {
        let sources = format!(
            "[[sources]]\nmount = \"{mount}\"\ntransport = \"http\"\nbase_url = \"https://x.example.test\"\n"
        );
        assert_rejected(&config_with(&[], FORMATS, &sources), "mount");
    }
    // 32 characters is the maximum valid length.
    let max_mount = "a".repeat(32);
    let sources = format!(
        "[[sources]]\nmount = \"{max_mount}\"\ntransport = \"http\"\nbase_url = \"https://x.example.test\"\n"
    );
    load(&config_with(&[], FORMATS, &sources)).expect("32-character mount is valid");
}

/// The full documented mount alphabet is valid after the first letter:
/// digits and hyphens, not only lowercase letters.
#[test]
fn mount_names_with_digits_and_hyphens_are_valid() {
    for mount in ["cdn2", "a-1", "img-archive-01", "p0-q1-r2"] {
        let sources = format!(
            "[[sources]]\nmount = \"{mount}\"\ntransport = \"http\"\nbase_url = \"https://x.example.test\"\n"
        );
        let config = load(&config_with(&[], FORMATS, &sources))
            .unwrap_or_else(|err| panic!("mount {mount:?} must be valid: {err}"));
        assert!(
            config.source_for_mount(mount).is_some(),
            "mount {mount:?} must be registered"
        );
    }
}

#[test]
fn duplicate_mounts_are_rejected() {
    let sources = format!("{}\n{}", http_source(""), http_source(""));
    let toml = config_with(&[], FORMATS, &sources);
    let err = load(&toml).expect_err("duplicate mounts must not silently replace one another");
    let message = err.to_string();
    assert!(
        message.contains("pics"),
        "message {message:?} should name the mount"
    );
    assert!(
        message.contains("duplicate") || message.contains("unique"),
        "message {message:?} should say the mount is duplicated"
    );
}

#[test]
fn unknown_transports_are_rejected() {
    for transport in ["ftp", "HTTP", "S3", "file", ""] {
        let sources = format!(
            "[[sources]]\nmount = \"pics\"\ntransport = \"{transport}\"\nbase_url = \"https://x.example.test\"\n"
        );
        assert_rejected(&config_with(&[], FORMATS, &sources), "transport");
    }
}

#[test]
fn missing_transport_specific_required_fields_are_rejected() {
    let cases = [
        (
            "[[sources]]\nmount = \"m\"\ntransport = \"http\"\n",
            "base_url",
        ),
        (
            "[[sources]]\nmount = \"m\"\ntransport = \"filesystem\"\n",
            "root",
        ),
        (
            "[[sources]]\nmount = \"m\"\ntransport = \"s3\"\nregion = \"us-east-1\"\n",
            "bucket",
        ),
        (
            "[[sources]]\nmount = \"m\"\ntransport = \"s3\"\nbucket = \"b\"\n",
            "region",
        ),
    ];
    for (sources, field) in cases {
        assert_rejected(&config_with(&[], FORMATS, sources), field);
    }
}

#[test]
fn transport_foreign_fields_are_mutually_exclusive() {
    let cases: [(String, &str); 14] = [
        // http cannot carry filesystem or s3 fields.
        (http_source("root = \".\"\n"), "root"),
        (http_source("bucket = \"b\"\n"), "bucket"),
        (http_source("region = \"us-east-1\"\n"), "region"),
        (
            http_source("endpoint_url = \"http://localhost:9000\"\n"),
            "endpoint_url",
        ),
        (http_source("force_path_style = true\n"), "force_path_style"),
        // filesystem cannot carry http or s3 fields.
        (
            filesystem_source("base_url = \"https://x.example.test\"\n"),
            "base_url",
        ),
        (
            filesystem_source("ca_certificate_file = \"ca.pem\"\n"),
            "ca_certificate_file",
        ),
        (filesystem_source("bucket = \"b\"\n"), "bucket"),
        (filesystem_source("region = \"us-east-1\"\n"), "region"),
        (
            filesystem_source("endpoint_url = \"http://localhost:9000\"\n"),
            "endpoint_url",
        ),
        (
            filesystem_source("force_path_style = true\n"),
            "force_path_style",
        ),
        // s3 cannot carry http or filesystem fields.
        (
            s3_source("base_url = \"https://x.example.test\"\n"),
            "base_url",
        ),
        (
            s3_source("ca_certificate_file = \"ca.pem\"\n"),
            "ca_certificate_file",
        ),
        (s3_source("root = \".\"\n"), "root"),
    ];
    for (sources, field) in &cases {
        assert_rejected(&config_with(&[], FORMATS, sources), field);
    }
}

#[test]
fn credentials_in_toml_are_rejected() {
    for credential in ["access_key_id", "secret_access_key", "session_token"] {
        let sources = s3_source(&format!("{credential} = \"AKIAEXAMPLE\"\n"));
        assert_rejected(&config_with(&[], FORMATS, &sources), credential);
    }
}

#[test]
fn unknown_source_fields_are_rejected() {
    let sources = http_source("verify_tls = false\n");
    assert_rejected(&config_with(&[], FORMATS, &sources), "verify_tls");
}

// ---------------------------------------------------------------------------
// HTTP(S) source specifics.
// ---------------------------------------------------------------------------

#[test]
fn base_url_scheme_must_be_http_or_https() {
    for base_url in ["ftp://x.example.test", "file:///etc", "gopher://x"] {
        let sources = format!(
            "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"{base_url}\"\n"
        );
        assert_rejected(&config_with(&[], FORMATS, &sources), "base_url");
    }
    let sources = "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"not a url\"\n";
    assert_rejected(&config_with(&[], FORMATS, sources), "base_url");

    for base_url in ["http://images.example.test", "https://images.example.test"] {
        let sources = format!(
            "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"{base_url}\"\n"
        );
        load(&config_with(&[], FORMATS, &sources))
            .unwrap_or_else(|e| panic!("{base_url} should be accepted: {e}"));
    }
}

/// Private, loopback, link-local, and metadata-style base URLs are startup
/// errors unless the source explicitly opts in. All are fake or reserved
/// addresses; nothing is contacted during config validation.
#[test]
fn private_or_local_base_urls_are_rejected_without_the_opt_in() {
    let blocked = [
        "http://127.0.0.1:9000",
        "http://127.8.9.10",
        "https://[::1]:8443",
        "http://0.0.0.0",
        "http://169.254.169.254/latest",
        "http://[fe80::1]",
        "http://[fd00:ec2::254]",
        "http://[::ffff:169.254.169.254]",
        "http://10.0.0.7",
        "http://172.16.3.4",
        "http://192.168.1.10",
        "http://100.100.100.200",
        "http://localhost:9000",
        "http://images.localhost",
        "http://metadata.google.internal",
        "http://metadata",
        "http://something.internal",
    ];
    for base_url in blocked {
        let sources = format!(
            "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"{base_url}\"\n"
        );
        assert_rejected(&config_with(&[], FORMATS, &sources), "base_url");
        // The error must point at the remedy.
        let err = load(&config_with(&[], FORMATS, &sources)).unwrap_err();
        assert!(
            err.to_string().contains("allow_private_destinations"),
            "error for {base_url} should mention the opt-in, got {err}"
        );
    }
}

#[test]
fn private_base_urls_are_accepted_with_the_explicit_opt_in() {
    for base_url in ["http://127.0.0.1:9000", "http://localhost:9000"] {
        let sources = format!(
            "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\nbase_url = \"{base_url}\"\n\
             allow_private_destinations = true\n"
        );
        let config = load(&config_with(&[], FORMATS, &sources))
            .unwrap_or_else(|e| panic!("{base_url} with the opt-in should be accepted: {e}"));
        match only_source(&config) {
            SourceConfig::Http(http) => assert!(http.allow_private_destinations),
            other => panic!("expected an http source, got {other:?}"),
        }
    }
}

#[test]
fn public_base_urls_default_to_the_destination_policy_being_enforced() {
    let config = load(&valid_config()).expect("baseline configuration must parse");
    match only_source(&config) {
        SourceConfig::Http(http) => assert!(!http.allow_private_destinations),
        other => panic!("expected an http source, got {other:?}"),
    }
}

#[test]
fn ca_certificate_file_requires_https_base_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("ca.pem"), "dummy pem\n").expect("write ca");
    let sources = "[[sources]]\nmount = \"pics\"\ntransport = \"http\"\n\
                   base_url = \"http://images.example.test\"\n\
                   ca_certificate_file = \"ca.pem\"\n";
    let toml = config_with(&[], FORMATS, sources);
    let err = load_from_str(&toml, dir.path()).expect_err("http + CA file must be rejected");
    assert!(err.to_string().contains("ca_certificate_file"));
}

#[test]
fn ca_certificate_file_resolves_relative_to_config_dir_and_must_be_readable() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("ca.pem"), "dummy pem\n").expect("write ca");
    let sources = http_source("ca_certificate_file = \"ca.pem\"\n");
    let toml = config_with(&[], FORMATS, &sources);

    let config = load_from_str(&toml, dir.path()).expect("existing CA file must be accepted");
    match only_source(&config) {
        SourceConfig::Http(http) => {
            let ca = http.ca_certificate_file.as_ref().expect("stored CA path");
            assert_eq!(ca, &dir.path().join("ca.pem"));
        }
        other => panic!("expected an http source, got {other:?}"),
    }

    // A missing CA file is a startup error, not a deferred failure.
    let missing = http_source("ca_certificate_file = \"nope.pem\"\n");
    let toml = config_with(&[], FORMATS, &missing);
    let err = load_from_str(&toml, dir.path()).expect_err("missing CA file must be rejected");
    assert!(err.to_string().contains("ca_certificate_file"));

    // A directory is not a usable CA file.
    std::fs::create_dir(dir.path().join("cadir")).expect("mkdir");
    let dir_ca = http_source("ca_certificate_file = \"cadir\"\n");
    let toml = config_with(&[], FORMATS, &dir_ca);
    let err = load_from_str(&toml, dir.path()).expect_err("directory CA path must be rejected");
    assert!(err.to_string().contains("ca_certificate_file"));
}

// ---------------------------------------------------------------------------
// S3 source specifics.
// ---------------------------------------------------------------------------

#[test]
fn s3_bucket_and_region_are_validated() {
    let cases = [
        ("bucket = \"\"\nregion = \"us-east-1\"", "bucket"),
        ("bucket = \"a/b\"\nregion = \"us-east-1\"", "bucket"),
        ("bucket = \"a\\u0000b\"\nregion = \"us-east-1\"", "bucket"),
        ("bucket = \"b\"\nregion = \"\"", "region"),
        ("bucket = \"b\"\nregion = \"us/east\"", "region"),
        ("bucket = \"b\"\nregion = \"us\\u0007east\"", "region"),
    ];
    for (fields, field) in cases {
        let sources = format!("[[sources]]\nmount = \"m\"\ntransport = \"s3\"\n{fields}\n");
        assert_rejected(&config_with(&[], FORMATS, &sources), field);
    }
}

#[test]
fn s3_endpoint_url_scheme_must_be_http_or_https() {
    let bad = s3_source("endpoint_url = \"ftp://localhost:9000\"\n");
    assert_rejected(&config_with(&[], FORMATS, &bad), "endpoint_url");

    let good = s3_source(
        "endpoint_url = \"http://localhost:9000\"\nforce_path_style = true\n\
         allow_private_destinations = true\n",
    );
    let config = load(&config_with(&[], FORMATS, &good)).expect("local endpoint must parse");
    match only_source(&config) {
        SourceConfig::S3(s3) => {
            assert_eq!(
                s3.endpoint_url.as_ref().map(|u| u.as_str()),
                Some("http://localhost:9000/")
            );
            assert!(s3.force_path_style);
        }
        other => panic!("expected an s3 source, got {other:?}"),
    }
}

/// A private or local S3 `endpoint_url` needs the same explicit opt-in as
/// an http `base_url`; a public endpoint needs none.
#[test]
fn s3_endpoint_url_destination_policy_matches_the_http_one() {
    for endpoint in [
        "http://localhost:9000",
        "http://127.0.0.1:9000",
        "http://169.254.169.254",
    ] {
        let sources = s3_source(&format!("endpoint_url = \"{endpoint}\"\n"));
        assert_rejected(&config_with(&[], FORMATS, &sources), "endpoint_url");
    }

    let public = s3_source("endpoint_url = \"https://s3.example.test\"\n");
    load(&config_with(&[], FORMATS, &public)).expect("public endpoint needs no opt-in");
}

/// The opt-in only makes sense where an upstream URL exists: never on
/// filesystem sources, and only next to `endpoint_url` on s3 sources.
#[test]
fn allow_private_destinations_requires_an_upstream_url() {
    let on_filesystem = filesystem_source("allow_private_destinations = true\n");
    assert_rejected(
        &config_with(&[], FORMATS, &on_filesystem),
        "allow_private_destinations",
    );

    let on_s3_without_endpoint = s3_source("allow_private_destinations = true\n");
    assert_rejected(
        &config_with(&[], FORMATS, &on_s3_without_endpoint),
        "allow_private_destinations",
    );
}

// ---------------------------------------------------------------------------
// Key Prefix semantics.
// ---------------------------------------------------------------------------

#[test]
fn key_prefix_defaults_to_the_mount_when_omitted() {
    let config = load(&config_with(&[], FORMATS, &http_source(""))).expect("must parse");
    assert_eq!(only_source(&config).key_prefix_segments(), ["pics"]);
}

#[test]
fn empty_key_prefix_means_no_prefix_segments() {
    let sources = http_source("key_prefix = \"\"\n");
    let config = load(&config_with(&[], FORMATS, &sources)).expect("must parse");
    assert!(only_source(&config).key_prefix_segments().is_empty());
}

#[test]
fn custom_key_prefix_is_split_into_segments() {
    let cases: [(&str, &[&str]); 3] = [
        ("media", &["media"]),
        ("a/b/c", &["a", "b", "c"]),
        ("dotted.dir/img_v2", &["dotted.dir", "img_v2"]),
    ];
    for (prefix, expected) in cases {
        let sources = http_source(&format!("key_prefix = \"{prefix}\"\n"));
        let config = load(&config_with(&[], FORMATS, &sources))
            .unwrap_or_else(|e| panic!("key_prefix {prefix:?} should be valid: {e}"));
        assert_eq!(only_source(&config).key_prefix_segments(), expected);
    }
}

#[test]
fn traversal_and_non_canonical_key_prefixes_are_rejected() {
    let invalid = [
        "..", ".", "a/..", "../a", "a/../b", "./a", "a/./b", "a//b", "/a", "a/",
    ];
    for prefix in invalid {
        let sources = http_source(&format!("key_prefix = \"{prefix}\"\n"));
        assert_rejected(&config_with(&[], FORMATS, &sources), "key_prefix");
    }
}

// ---------------------------------------------------------------------------
// Filesystem root resolution.
// ---------------------------------------------------------------------------

fn filesystem_source_with_root(root: &str) -> String {
    format!("[[sources]]\nmount = \"local\"\ntransport = \"filesystem\"\nroot = \"{root}\"\n")
}

#[test]
fn relative_root_resolves_against_config_file_dir_not_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("imgs")).expect("mkdir");
    let config_path = dir.path().join("service.toml");
    std::fs::write(
        &config_path,
        config_with(&[], FORMATS, &filesystem_source_with_root("./imgs")),
    )
    .expect("write config");

    // The process CWD has no `imgs` directory, so this only succeeds when
    // the root resolves against the configuration file's directory.
    assert!(
        !Path::new("imgs").exists(),
        "test precondition: CWD must not contain an `imgs` directory"
    );

    let config = load_from_file(&config_path).expect("relative root must resolve");
    match only_source(&config) {
        SourceConfig::Filesystem(fs) => {
            let expected = std::fs::canonicalize(dir.path().join("imgs")).expect("canonicalize");
            assert_eq!(fs.root, expected);
            assert!(fs.root.is_absolute());
        }
        other => panic!("expected a filesystem source, got {other:?}"),
    }
}

#[test]
fn load_from_str_resolves_relative_root_against_base_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("media")).expect("mkdir");
    let toml = config_with(&[], FORMATS, &filesystem_source_with_root("media"));
    let config = load_from_str(&toml, dir.path()).expect("relative root must resolve");
    match only_source(&config) {
        SourceConfig::Filesystem(fs) => {
            let expected = std::fs::canonicalize(dir.path().join("media")).expect("canonicalize");
            assert_eq!(fs.root, expected);
        }
        other => panic!("expected a filesystem source, got {other:?}"),
    }
}

#[test]
fn nonexistent_or_non_directory_root_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");

    let toml = config_with(&[], FORMATS, &filesystem_source_with_root("./missing"));
    let err = load_from_str(&toml, dir.path()).expect_err("missing root must be rejected");
    assert!(err.to_string().contains("root"));

    std::fs::write(dir.path().join("afile"), b"not a directory").expect("write file");
    let toml = config_with(&[], FORMATS, &filesystem_source_with_root("./afile"));
    let err = load_from_str(&toml, dir.path()).expect_err("file root must be rejected");
    assert!(err.to_string().contains("root"));
}

#[test]
fn unreadable_configuration_file_is_a_config_error() {
    let err = load_from_file(Path::new("/definitely/missing/config.toml"))
        .expect_err("missing config file must be an error");
    assert!(err.to_string().contains("config"));
}
