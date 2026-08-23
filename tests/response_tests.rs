//! Response and error contract tests.
//!
//! Drives the real router over a real listening socket: the app is built
//! in-process (`build_router` + `axum::serve` on an ephemeral listener) and
//! exercised with a tiny raw HTTP/1.1 client over tokio TcpStream, so the
//! asserted statuses, headers, and bodies are exactly what a caller sees on
//! the wire.
//!
//! Completion-log shape is asserted end-to-end in `tests/e2e_tests.rs`
//! against the real binary's stdout; this file owns statuses, headers, and
//! bodies only.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use image::RgbImage;
use pixtega::app::{build_router, AppState};
use pixtega::config::{
    AppConfig, FilesystemSourceConfig, FormatPolicy, HttpSourceConfig, SourceConfig,
};
use pixtega::errors::ProcessError;
use pixtega::processor::init_vips;
use pixtega::sources::SourceRegistry;
use pixtega::types::OutputFormat;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

// ---------------------------------------------------------------------------
// Tiny raw HTTP client
// ---------------------------------------------------------------------------

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == lower)
            .map(|(_, v)| v.as_str())
    }
}

async fn send_request(addr: SocketAddr, method: &str, target: &str) -> HttpResponse {
    let request =
        format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let raw = tokio::time::timeout(Duration::from_secs(30), async {
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("read");
        raw
    })
    .await
    .expect("response within 30s");
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> HttpResponse {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    let headers = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    HttpResponse {
        status,
        headers,
        body: raw[head_end + 4..].to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Fixture HTTP server (upstream Source)
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Script {
    /// Write these raw bytes, then close.
    Bytes(Vec<u8>),
    /// Read the request, never respond.
    Stall,
}

async fn start_fixture(routes: HashMap<String, Script>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 2048];
                loop {
                    let Ok(read) = stream.read(&mut buf).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..read]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&head);
                let target = text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                match routes.get(&target).cloned() {
                    Some(Script::Bytes(bytes)) => {
                        let _ = stream.write_all(&bytes).await;
                    }
                    Some(Script::Stall) => {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    None => {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                    }
                }
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

fn upstream_response(status: u16, reason: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

// ---------------------------------------------------------------------------
// App under test
// ---------------------------------------------------------------------------

const UNVERSIONED_TTL: u64 = 777;
const NOT_FOUND_TTL: u64 = 55;

fn test_config(sources: Vec<SourceConfig>, download_timeout_ms: u64) -> AppConfig {
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
            allowed_qualities: vec![70, 92],
        },
    );
    AppConfig {
        listen_address: "127.0.0.1:0".parse().unwrap(),
        path_prefix: "/images".to_string(),
        allowed_widths: vec![320, 640],
        max_download_bytes: 10 * 1024 * 1024,
        max_source_megapixels: 100,
        download_timeout_ms,
        max_redirects: 3,
        max_concurrent_derivations: 4,
        unversioned_success_ttl_seconds: UNVERSIONED_TTL,
        not_found_ttl_seconds: NOT_FOUND_TTL,
        formats,
        sources,
    }
}

fn filesystem_source(root: &std::path::Path) -> SourceConfig {
    SourceConfig::Filesystem(FilesystemSourceConfig {
        mount: "files".to_string(),
        key_prefix_segments: Vec::new(),
        root: root.canonicalize().expect("canonical fixture root"),
    })
}

fn http_source(port: u16) -> SourceConfig {
    SourceConfig::Http(HttpSourceConfig {
        mount: "pics".to_string(),
        key_prefix_segments: Vec::new(),
        base_url: Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
        ca_certificate_file: None,
        allow_private_destinations: true,
    })
}

async fn spawn_app(config: AppConfig) -> SocketAddr {
    init_vips();
    let registry = SourceRegistry::from_config(&config)
        .await
        .expect("registry construction");
    let state = AppState::new(config, registry);
    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    addr
}

/// A tempdir-backed filesystem Source with one real generated JPEG at
/// `photo.jpg`, plus the app serving it.
async fn spawn_filesystem_app() -> (tempfile::TempDir, SocketAddr) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("photo.jpg"), jpeg_fixture(800, 400)).unwrap();
    let addr = spawn_app(test_config(vec![filesystem_source(dir.path())], 10_000)).await;
    (dir, addr)
}

fn jpeg_fixture(width: u32, height: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            ((x * 255) / width.max(1)) as u8,
            ((y * 255) / height.max(1)) as u8,
            (((x + y) * 255) / (width + height).max(1)) as u8,
        ])
    });
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("fixture JPEG encode");
    buf.into_inner()
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

fn assert_security_headers(response: &HttpResponse) {
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        response.header("content-security-policy"),
        Some("default-src 'none'")
    );
    assert_eq!(response.header("x-frame-options"), Some("DENY"));
}

/// Every error body is JSON with a non-empty `error` string, no matter what
/// internal detail the failure carried: the body is built with serde_json,
/// so message content cannot corrupt it.
fn assert_error_body(response: &HttpResponse) {
    assert_eq!(response.header("content-type"), Some("application/json"));
    let parsed: serde_json::Value =
        serde_json::from_slice(&response.body).expect("error body parses as JSON");
    let message = parsed
        .get("error")
        .and_then(|v| v.as_str())
        .expect("`error` is a string");
    assert!(!message.is_empty(), "`error` must be non-empty");
}

fn assert_no_store(response: &HttpResponse) {
    assert_eq!(response.header("cache-control"), Some("no-store"));
}

// ---------------------------------------------------------------------------
// Successful responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn versioned_success_has_content_type_immutable_policy_and_security_headers() {
    let (_dir, addr) = spawn_filesystem_app().await;
    for (transform, content_type) in [
        ("w320.webp", "image/webp"),
        ("w320.avif", "image/avif"),
        ("w320.jpeg", "image/jpeg"),
    ] {
        let response = send_request(
            addr,
            "GET",
            &format!("/images/files/photo.jpg/{transform}?v=abc-123"),
        )
        .await;
        assert_eq!(response.status, 200, "transform {transform}");
        assert_eq!(
            response.header("content-type"),
            Some(content_type),
            "transform {transform}"
        );
        assert_eq!(
            response.header("cache-control"),
            Some("public, max-age=31536000, immutable"),
            "transform {transform}"
        );
        assert_security_headers(&response);
        assert!(!response.body.is_empty(), "derived image body");
    }
}

#[tokio::test]
async fn unversioned_success_uses_the_short_ttl_and_is_never_immutable() {
    let (_dir, addr) = spawn_filesystem_app().await;
    let response = send_request(addr, "GET", "/images/files/photo.jpg/w320.webp").await;
    assert_eq!(response.status, 200);
    let cache_control = response.header("cache-control").expect("cache-control");
    assert_eq!(cache_control, format!("public, max-age={UNVERSIONED_TTL}"));
    assert!(
        !cache_control.contains("immutable"),
        "an unversioned response must not include `immutable`, got {cache_control:?}"
    );
    assert_security_headers(&response);
    assert_eq!(&response.body[..4], b"RIFF", "WebP container signature");
}

// ---------------------------------------------------------------------------
// 404: absence, the only cacheable error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absence_is_a_cacheable_404_with_the_configured_ttl() {
    let (_dir, addr) = spawn_filesystem_app().await;
    let response = send_request(addr, "GET", "/images/files/nope.jpg/w320.webp?v=1").await;
    assert_eq!(response.status, 404);
    assert_eq!(
        response.header("cache-control"),
        Some(format!("public, max-age={NOT_FOUND_TTL}").as_str())
    );
    assert_error_body(&response);
    assert_security_headers(&response);
}

// ---------------------------------------------------------------------------
// Source failures: denial and timeout are non-cacheable 5xx, never 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_denial_is_a_502_and_cannot_be_mistaken_for_absence() {
    let port = start_fixture(HashMap::from([(
        "/denied.jpg".to_string(),
        Script::Bytes(upstream_response(403, "Forbidden", b"secret denial page")),
    )]))
    .await;
    let addr = spawn_app(test_config(vec![http_source(port)], 10_000)).await;

    let response = send_request(addr, "GET", "/images/pics/denied.jpg/w320.webp").await;
    assert_eq!(response.status, 502, "denial must be 502, not 404");
    assert_no_store(&response);
    assert_error_body(&response);
    assert_security_headers(&response);
    // The upstream body must never leak into the client-facing error.
    assert!(!String::from_utf8_lossy(&response.body).contains("secret denial page"));
}

#[tokio::test]
async fn upstream_500_is_a_non_cacheable_502() {
    let port = start_fixture(HashMap::from([(
        "/broken.jpg".to_string(),
        Script::Bytes(upstream_response(500, "Internal", b"boom")),
    )]))
    .await;
    let addr = spawn_app(test_config(vec![http_source(port)], 10_000)).await;

    let response = send_request(addr, "GET", "/images/pics/broken.jpg/w320.webp").await;
    assert_eq!(response.status, 502);
    assert_no_store(&response);
    assert_error_body(&response);
}

#[tokio::test]
async fn source_timeout_is_a_non_cacheable_504() {
    let port = start_fixture(HashMap::from([("/slow.jpg".to_string(), Script::Stall)])).await;
    let addr = spawn_app(test_config(vec![http_source(port)], 300)).await;

    let response = send_request(addr, "GET", "/images/pics/slow.jpg/w320.webp").await;
    assert_eq!(response.status, 504);
    assert_no_store(&response);
    assert_error_body(&response);
    assert_security_headers(&response);
}

// ---------------------------------------------------------------------------
// Processing failures: source bytes vs the service's own pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn undecodable_source_bytes_are_a_non_cacheable_502() {
    let port = start_fixture(HashMap::from([(
        "/note.txt".to_string(),
        Script::Bytes(upstream_response(200, "OK", b"this is not an image at all")),
    )]))
    .await;
    let addr = spawn_app(test_config(vec![http_source(port)], 10_000)).await;

    let response = send_request(addr, "GET", "/images/pics/note.txt/w320.webp").await;
    assert_eq!(
        response.status, 502,
        "bad source bytes are a source problem"
    );
    assert_no_store(&response);
    assert_error_body(&response);
}

/// A real pipeline 500 (resize/flatten/encode failing AFTER a valid source
/// image was accepted) cannot be triggered through the HTTP surface without
/// breaking the process, so the 502/500 split is asserted on the error
/// taxonomy itself: invalid source bytes are 502 (verified over HTTP above),
/// while pipeline failures carry 500.
#[test]
fn processing_taxonomy_distinguishes_source_bytes_from_pipeline_failures() {
    let undecodable = ProcessError::Undecodable {
        detail: "x".to_string(),
    };
    assert_eq!(undecodable.status(), 502);
    for pipeline in [
        ProcessError::Resize {
            detail: "x".to_string(),
        },
        ProcessError::Flatten {
            detail: "x".to_string(),
        },
        ProcessError::Encode {
            detail: "x".to_string(),
        },
    ] {
        assert_eq!(pipeline.status(), 500, "{pipeline:?}");
    }
}

// ---------------------------------------------------------------------------
// Invalid requests and methods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_requests_are_non_cacheable_400_json_errors() {
    let (_dir, addr) = spawn_filesystem_app().await;
    for target in [
        "/images/files/photo.jpg/w123.webp",     // width not in allowlist
        "/images/files/photo.jpg/w320.jpg",      // format alias
        "/images/files/photo.jpg/w320,q99.webp", // quality not in policy
        "/images/files/photo.jpg/w320,q82.webp", // quality equals default
        "/images/nope/photo.jpg/w320.webp",      // unknown mount
        "/images/files/%2E%2E/photo.jpg/w320.webp", // encoded traversal
        "/images/files/photo.jpg/w320.webp?v=1&v=2", // repeated v
        "/images/files/photo.jpg/w320.webp?x=1", // unknown query parameter
    ] {
        let response = send_request(addr, "GET", target).await;
        assert_eq!(response.status, 400, "target {target}");
        assert_no_store(&response);
        assert_error_body(&response);
        assert_security_headers(&response);
    }
}

#[tokio::test]
async fn non_get_methods_are_non_cacheable_405_with_allow_get() {
    let (_dir, addr) = spawn_filesystem_app().await;
    // HEAD is an optional extension this implementation does not provide,
    // so it must fall into the same 405 taxonomy as every non-GET method.
    for method in ["POST", "PUT", "DELETE", "HEAD"] {
        let response = send_request(addr, method, "/images/files/photo.jpg/w320.webp").await;
        assert_eq!(response.status, 405, "method {method}");
        assert_eq!(response.header("allow"), Some("GET"), "method {method}");
        assert_no_store(&response);
        if method == "HEAD" {
            // A HEAD response must not carry a body (hyper strips it), so
            // only the headers can be asserted.
            assert_eq!(response.header("content-type"), Some("application/json"));
            assert!(response.body.is_empty(), "HEAD response must have no body");
        } else {
            assert_error_body(&response);
        }
    }
}

/// An HTTP/1.1 absolute-form request target carries scheme and authority in
/// the target itself; the 8192-byte limit applies to the whole received
/// target, not only to path + query.
#[tokio::test]
async fn absolute_form_targets_over_8192_bytes_are_rejected() {
    let (_dir, addr) = spawn_filesystem_app().await;
    let long_host = format!("{}.test", "a".repeat(8300));
    let target = format!("http://{long_host}/images/files/photo.jpg/w320.webp");
    let response = send_request(addr, "GET", &target).await;
    assert_eq!(response.status, 400);
    assert_no_store(&response);
    assert_error_body(&response);

    // A short absolute-form target for the same resource still works.
    let target = format!(
        "http://127.0.0.1:{}/images/files/photo.jpg/w320.webp",
        addr.port()
    );
    let response = send_request(addr, "GET", &target).await;
    assert_eq!(response.status, 200);
}

/// The 8192-byte request-target limit is exclusive: a target of exactly
/// 8192 bytes passes the length gate (and fails later as absence, since the
/// named file does not exist); one more byte is rejected as overlong.
#[tokio::test]
async fn a_target_of_exactly_8192_bytes_passes_the_length_gate() {
    // An HTTP Source (whose fixture answers 404 for unknown paths) rather
    // than a filesystem Source: an 8000-byte file name would trip local
    // NAME_MAX/PATH_MAX limits and muddy the assertion with a 502.
    let port = start_fixture(HashMap::new()).await;
    let addr = spawn_app(test_config(vec![http_source(port)], 10_000)).await;
    let prefix = "/images/pics/";
    let suffix = "/w320.webp";
    let filler = 8192 - prefix.len() - suffix.len();

    let exact = format!("{prefix}{}{suffix}", "a".repeat(filler));
    assert_eq!(exact.len(), 8192);
    let response = send_request(addr, "GET", &exact).await;
    assert_eq!(response.status, 404, "exactly 8192 bytes is not overlong");

    let over = format!("{prefix}{}{suffix}", "a".repeat(filler + 1));
    let response = send_request(addr, "GET", &over).await;
    assert_eq!(response.status, 400, "8193 bytes is overlong");
    assert_no_store(&response);
    assert_error_body(&response);
}

/// The public error body is JSON-encoded, so hostile characters in an
/// internal message can never corrupt it.
#[test]
fn hostile_error_messages_cannot_corrupt_the_json_body() {
    for message in [
        "quote\" brace} newline\n nul\0 emoji🧨 backslash\\",
        "{\"error\":\"forged\"}",
        "\u{202e}control\u{7f}",
    ] {
        let body = pixtega::app::error_body(message);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body must be JSON");
        assert_eq!(parsed["error"].as_str(), Some(message));
        assert_eq!(parsed.as_object().map(|o| o.len()), Some(1));
    }
}
