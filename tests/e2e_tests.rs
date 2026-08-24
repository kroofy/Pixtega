//! End-to-end acceptance tests.
//!
//! Every test spawns the REAL service binary (`CARGO_BIN_EXE_pixtega`)
//! as a child process with its own tempdir-written TOML configuration, plus
//! local fixture servers on 127.0.0.1. The service listens on an ephemeral
//! port announced by its `{"event":"listening",...}` startup line.
//!
//! Tests are grouped into numbered acceptance items (the `Item N` comments
//! below). Item 11 (the container image) lives in
//! `scripts/container-acceptance.sh` because it needs Docker.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Cursor};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::{GenericImageView, RgbImage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Child process management
// ---------------------------------------------------------------------------

/// The running service binary. Killed on drop so a failing test cannot leak
/// listeners.
struct ChildGuard {
    child: Child,
    addr: SocketAddr,
    stdout_lines: Arc<Mutex<Vec<String>>>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Write `config_toml` into `dir` and start the real binary against it,
/// waiting for the startup line to learn the bound address. `envs` are set
/// on the child only (used for S3 credentials).
async fn spawn_service(dir: &Path, config_toml: &str, envs: &[(&str, &str)]) -> ChildGuard {
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, config_toml).expect("write config");

    let mut command = Command::new(env!("CARGO_BIN_EXE_pixtega"));
    command
        .arg(&config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in envs {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("spawn service binary");

    let stdout_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_sink = stdout_lines.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            stdout_sink.lock().unwrap().push(line);
        }
    });
    let stderr_sink = stderr_lines.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            stderr_sink.lock().unwrap().push(line);
        }
    });

    // Wait for the startup line announcing the actual bound address.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let addr = loop {
        if let Some(addr) = stdout_lines
            .lock()
            .unwrap()
            .iter()
            .find_map(|line| parse_listening_line(line))
        {
            break addr;
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "service exited during startup with {status}; stderr: {:?}",
                stderr_lines.lock().unwrap()
            );
        }
        if tokio::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!(
                "service never announced listening; stdout: {:?}, stderr: {:?}",
                stdout_lines.lock().unwrap(),
                stderr_lines.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    ChildGuard {
        child,
        addr,
        stdout_lines,
    }
}

fn parse_listening_line(line: &str) -> Option<SocketAddr> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("event")?.as_str()? != "listening" {
        return None;
    }
    value.get("address")?.as_str()?.parse().ok()
}

// ---------------------------------------------------------------------------
// Completion-log assertions
// ---------------------------------------------------------------------------

const CLOSED_OUTCOMES: [&str; 9] = [
    "success",
    "rejected_request",
    "not_found",
    "timeout",
    "source_too_large",
    "source_unavailable",
    "undecodable_source",
    "flatten_failed",
    "encode_failed",
];

fn completion_events(guard: &ChildGuard) -> Vec<serde_json::Value> {
    guard
        .stdout_lines
        .lock()
        .unwrap()
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("event").and_then(|e| e.as_str()) == Some("request_completed"))
        .collect()
}

/// Poll until at least `count` completion events were logged. The event is
/// emitted before the response is written, but the pipe reader may lag.
async fn wait_for_completions(guard: &ChildGuard, count: usize) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let events = completion_events(guard);
        if events.len() >= count {
            return events;
        }
        assert!(
            tokio::time::Instant::now() <= deadline,
            "expected {count} completion events, got {events:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Required fields and the closed outcome set, for every completion line.
fn assert_completion_shape(event: &serde_json::Value) {
    let status = event["status"].as_u64().expect("numeric status") as u16;
    assert!(
        [200, 400, 404, 405, 500, 502, 504].contains(&status),
        "status {status} outside the taxonomy"
    );
    let outcome = event["outcome"].as_str().expect("outcome string");
    assert!(
        CLOSED_OUTCOMES.contains(&outcome),
        "outcome {outcome:?} outside the closed set"
    );
    assert!(event["elapsed_ms"].as_u64().is_some(), "elapsed_ms");
}

fn assert_no_line_contains(guard: &ChildGuard, needle: &str) {
    for line in guard.stdout_lines.lock().unwrap().iter() {
        assert!(
            !line.contains(needle),
            "log line must never contain {needle:?}: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// Raw HTTP client
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
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
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
// Fixture server
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Script {
    /// Write these raw bytes, then close.
    Bytes(Vec<u8>),
    /// Read the request, never respond.
    Stall,
    /// 200 with chunked transfer encoding, no Content-Length, `total` bytes.
    ChunkedHuge { total: usize },
    /// Sleep, then write the raw bytes. Used by the concurrency test.
    DelayedBytes { bytes: Vec<u8>, delay_ms: u64 },
}

struct Fixture {
    port: u16,
    /// Full request targets (path + query) in arrival order.
    hits: Arc<Mutex<Vec<String>>>,
    /// Highest number of concurrently in-flight requests observed.
    inflight_max: Arc<AtomicUsize>,
    /// Body bytes actually written for `ChunkedHuge` responses.
    chunked_sent: Arc<AtomicUsize>,
}

impl Fixture {
    async fn start(routes: HashMap<String, Script>) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let inflight = Arc::new(AtomicUsize::new(0));
        let inflight_max = Arc::new(AtomicUsize::new(0));
        let chunked_sent = Arc::new(AtomicUsize::new(0));
        let routes = Arc::new(routes);
        {
            let hits = hits.clone();
            let inflight = inflight.clone();
            let inflight_max = inflight_max.clone();
            let chunked_sent = chunked_sent.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let hits = hits.clone();
                    let inflight = inflight.clone();
                    let inflight_max = inflight_max.clone();
                    let chunked_sent = chunked_sent.clone();
                    let routes = routes.clone();
                    tokio::spawn(async move {
                        let Some(target) = read_request_target(&mut stream).await else {
                            return;
                        };
                        let path = target.split('?').next().unwrap_or("").to_string();
                        let script = routes.get(&path).cloned();
                        hits.lock().unwrap().push(target);
                        let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                        inflight_max.fetch_max(current, Ordering::SeqCst);
                        match script {
                            Some(Script::Bytes(bytes)) => {
                                let _ = stream.write_all(&bytes).await;
                            }
                            Some(Script::Stall) => {
                                tokio::time::sleep(Duration::from_secs(60)).await;
                            }
                            Some(Script::ChunkedHuge { total }) => {
                                let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                                let _ = stream.write_all(head).await;
                                let chunk = vec![b'x'; 8192];
                                let mut sent = 0usize;
                                while sent < total {
                                    let n = chunk.len().min(total - sent);
                                    let framed = format!(
                                        "{n:x}\r\n{}\r\n",
                                        String::from_utf8_lossy(&chunk[..n])
                                    );
                                    if stream.write_all(framed.as_bytes()).await.is_err() {
                                        break;
                                    }
                                    sent += n;
                                    chunked_sent.store(sent, Ordering::SeqCst);
                                }
                                let _ = stream.write_all(b"0\r\n\r\n").await;
                            }
                            Some(Script::DelayedBytes { bytes, delay_ms }) => {
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                                let _ = stream.write_all(&bytes).await;
                            }
                            None => {
                                let _ = stream
                                    .write_all(
                                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                    )
                                    .await;
                            }
                        }
                        inflight.fetch_sub(1, Ordering::SeqCst);
                        let _ = stream.shutdown().await;
                    });
                }
            });
        }
        Fixture {
            port,
            hits,
            inflight_max,
            chunked_sent,
        }
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head: Vec<u8> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let read = stream.read(&mut buf).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&buf[..read]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() > 65536 {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let request_line = text.split("\r\n").next()?;
    Some(request_line.split_whitespace().nth(1)?.to_string())
}

fn ok_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn status_response(code: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

// ---------------------------------------------------------------------------
// Image fixtures and output inspection
// ---------------------------------------------------------------------------

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

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis")
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[..2] == [0xFF, 0xD8]
}

fn decode(bytes: &[u8]) -> image::DynamicImage {
    image::load_from_memory(bytes).expect("derived image decodes")
}

// ---------------------------------------------------------------------------
// Configuration text
// ---------------------------------------------------------------------------

const UNVERSIONED_TTL: u64 = 900;
const NOT_FOUND_TTL: u64 = 66;

struct ConfigOptions {
    max_download_bytes: u64,
    download_timeout_ms: u64,
    max_concurrent_derivations: usize,
}

impl Default for ConfigOptions {
    fn default() -> Self {
        ConfigOptions {
            max_download_bytes: 10 * 1024 * 1024,
            download_timeout_ms: 10_000,
            max_concurrent_derivations: 8,
        }
    }
}

/// Shared config head; `sources_toml` supplies the `[[sources]]` blocks.
fn config_toml(options: &ConfigOptions, sources_toml: &str) -> String {
    format!(
        r#"listen_address = "127.0.0.1:0"
path_prefix = "/images"

allowed_widths = [320, 640, 1280]

max_download_bytes = {max_download_bytes}
max_source_megapixels = 100
download_timeout_ms = {download_timeout_ms}
max_redirects = 3
max_concurrent_derivations = {max_concurrent_derivations}
unversioned_success_ttl_seconds = {UNVERSIONED_TTL}
not_found_ttl_seconds = {NOT_FOUND_TTL}

[formats.webp]
default_quality = 82
allowed_qualities = [60, 72, 90]

[formats.avif]
default_quality = 55
allowed_qualities = [40, 65]

[formats.jpeg]
default_quality = 85
allowed_qualities = [70, 92]

{sources_toml}
"#,
        max_download_bytes = options.max_download_bytes,
        download_timeout_ms = options.download_timeout_ms,
        max_concurrent_derivations = options.max_concurrent_derivations,
    )
}

fn http_source_toml(port: u16) -> String {
    format!(
        r#"[[sources]]
mount = "pub"
key_prefix = ""
transport = "http"
base_url = "http://127.0.0.1:{port}"
allow_private_destinations = true
"#
    )
}

/// Spawn the service with one plain-HTTP source pointed at `fixture`.
async fn spawn_http_service(dir: &Path, fixture: &Fixture, options: ConfigOptions) -> ChildGuard {
    let config = config_toml(&options, &http_source_toml(fixture.port));
    spawn_service(dir, &config, &[]).await
}

// ---------------------------------------------------------------------------
// Items 1-4: derivation formats, aspect ratio, no upscaling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_jpeg_source_derives_webp_avif_and_jpeg_preserving_aspect_ratio() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/pic.jpg".to_string(),
        Script::Bytes(ok_response(&jpeg_fixture(800, 400))),
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    let version = "vtok123secret";

    // Item 1: source JPEG -> smaller WebP.
    let webp = send_request(
        service.addr,
        "GET",
        &format!("/images/pub/photos/pic.jpg/w320.webp?v={version}"),
    )
    .await;
    assert_eq!(webp.status, 200);
    assert_eq!(webp.header("content-type"), Some("image/webp"));
    assert!(is_webp(&webp.body), "WebP container signature");
    let decoded = decode(&webp.body);
    // Item 3: aspect ratio preserved (800x400 -> 320x160).
    assert_eq!(decoded.dimensions(), (320, 160));
    assert!(
        webp.body.len() < jpeg_fixture(800, 400).len(),
        "derived variant is smaller than the source"
    );

    // Item 2: the same source as AVIF and JPEG.
    let avif = send_request(
        service.addr,
        "GET",
        &format!("/images/pub/photos/pic.jpg/w320.avif?v={version}"),
    )
    .await;
    assert_eq!(avif.status, 200);
    assert_eq!(avif.header("content-type"), Some("image/avif"));
    assert!(is_avif(&avif.body), "AVIF ftyp brand");

    let jpeg = send_request(
        service.addr,
        "GET",
        &format!("/images/pub/photos/pic.jpg/w320.jpeg?v={version}"),
    )
    .await;
    assert_eq!(jpeg.status, 200);
    assert_eq!(jpeg.header("content-type"), Some("image/jpeg"));
    assert!(is_jpeg(&jpeg.body), "JPEG SOI marker");
    assert_eq!(decode(&jpeg.body).dimensions(), (320, 160));

    // Completion logs: one line per request, required fields, closed
    // outcomes, and never the version token value.
    let events = wait_for_completions(&service, 3).await;
    assert_eq!(events.len(), 3, "exactly one completion event per request");
    for (event, format) in events.iter().zip(["webp", "avif", "jpeg"]) {
        assert_completion_shape(event);
        assert_eq!(event["status"], 200);
        assert_eq!(event["outcome"], "success");
        assert_eq!(event["mount"], "pub");
        assert_eq!(event["width"], 320);
        assert_eq!(event["format"], format);
        assert_eq!(event["upstream_status"], 200);
        assert!(event["input_bytes"].as_u64().unwrap() > 0);
        assert!(event["output_bytes"].as_u64().unwrap() > 0);
    }
    assert_no_line_contains(&service, version);
}

#[tokio::test]
async fn e2e_source_narrower_than_the_target_is_not_enlarged() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/small.jpg".to_string(),
        Script::Bytes(ok_response(&jpeg_fixture(200, 100))),
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    let response = send_request(
        service.addr,
        "GET",
        "/images/pub/photos/small.jpg/w640.webp?v=1",
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(decode(&response.body).dimensions(), (200, 100));
}

// ---------------------------------------------------------------------------
// Items 5-7: absence, denial, slowness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_missing_source_returns_a_cacheable_404() {
    let fixture = Fixture::start(HashMap::new()).await; // everything 404s
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    let response = send_request(
        service.addr,
        "GET",
        "/images/pub/photos/absent.jpg/w320.webp?v=1",
    )
    .await;
    assert_eq!(response.status, 404);
    assert_eq!(
        response.header("cache-control"),
        Some(format!("public, max-age={NOT_FOUND_TTL}").as_str())
    );
    let events = wait_for_completions(&service, 1).await;
    assert_completion_shape(&events[0]);
    assert_eq!(events[0]["outcome"], "not_found");
}

#[tokio::test]
async fn e2e_denied_source_returns_a_non_cacheable_502() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/locked.jpg".to_string(),
        Script::Bytes(status_response(403, "Forbidden")),
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    let response = send_request(
        service.addr,
        "GET",
        "/images/pub/photos/locked.jpg/w320.webp",
    )
    .await;
    assert_eq!(response.status, 502, "denial is 502, never 404");
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let events = wait_for_completions(&service, 1).await;
    assert_completion_shape(&events[0]);
    assert_eq!(events[0]["outcome"], "source_unavailable");
    assert_eq!(events[0]["upstream_status"], 403);
}

#[tokio::test]
async fn e2e_slow_source_returns_a_non_cacheable_504() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/slow.jpg".to_string(),
        Script::Stall,
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let options = ConfigOptions {
        download_timeout_ms: 300,
        ..ConfigOptions::default()
    };
    let service = spawn_http_service(dir.path(), &fixture, options).await;

    let response = send_request(service.addr, "GET", "/images/pub/photos/slow.jpg/w320.webp").await;
    assert_eq!(response.status, 504);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let events = wait_for_completions(&service, 1).await;
    assert_completion_shape(&events[0]);
    assert_eq!(events[0]["outcome"], "timeout");
}

// ---------------------------------------------------------------------------
// Item 8: streamed over-limit bodies are stopped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_streamed_response_over_the_byte_limit_is_stopped() {
    let total = 32 * 1024 * 1024;
    let fixture = Fixture::start(HashMap::from([(
        "/photos/huge.jpg".to_string(),
        Script::ChunkedHuge { total },
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let options = ConfigOptions {
        max_download_bytes: 64 * 1024,
        ..ConfigOptions::default()
    };
    let service = spawn_http_service(dir.path(), &fixture, options).await;

    let response = send_request(service.addr, "GET", "/images/pub/photos/huge.jpg/w320.webp").await;
    assert_eq!(response.status, 502);
    assert_eq!(response.header("cache-control"), Some("no-store"));

    let events = wait_for_completions(&service, 1).await;
    assert_completion_shape(&events[0]);
    assert_eq!(events[0]["outcome"], "source_too_large");

    // The service stopped reading: give the fixture a moment to hit the
    // write error, then check it never streamed anywhere near the total.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sent = fixture.chunked_sent.load(Ordering::SeqCst);
    assert!(
        sent < total / 2,
        "download must stop early: fixture wrote {sent} of {total} bytes"
    );
}

// ---------------------------------------------------------------------------
// Item 9: invalid requests perform no Source I/O
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_invalid_requests_never_reach_the_fixture_server() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/pic.jpg".to_string(),
        Script::Bytes(ok_response(&jpeg_fixture(64, 64))),
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    for target in [
        "/images/pub/photos/pic.jpg/w999.webp",    // width not allowed
        "/images/pub/photos/pic.jpg/w320.gif",     // unknown format
        "/images/pub/photos/pic.jpg/w320.jpg",     // format alias
        "/images/pub/photos/pic.jpg/w320,q1.webp", // quality not allowed
        "/images/pub/photos/pic.jpg/w320,q82.webp", // quality equals default
        "/images/nosuch/photos/pic.jpg/w320.webp", // unknown mount
        "/images/pub/%2E%2E/pic.jpg/w320.webp",    // encoded traversal
        "/images/pub/photos/pic.jpg/w320.webp?v=%41", // encoded v
        "/images/pub/photos/pic.jpg/w320.webp?bad=1", // unknown query
    ] {
        let response = send_request(service.addr, "GET", target).await;
        assert_eq!(response.status, 400, "target {target}");
    }
    let events = wait_for_completions(&service, 9).await;
    for event in &events {
        assert_completion_shape(event);
        assert_eq!(event["outcome"], "rejected_request");
    }
    assert!(
        fixture.hits().is_empty(),
        "invalid requests must perform no Source I/O, saw {:?}",
        fixture.hits()
    );
}

// ---------------------------------------------------------------------------
// Item 10: traversal cannot read a sentinel outside the filesystem root
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_traversal_cannot_read_a_sentinel_outside_the_filesystem_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("ok.jpg"), jpeg_fixture(64, 64)).unwrap();
    let sentinel = "SENTINEL-9f8e7d6c-do-not-serve";
    std::fs::write(dir.path().join("sentinel.txt"), sentinel).unwrap();

    let config = config_toml(
        &ConfigOptions::default(),
        r#"[[sources]]
mount = "fs"
key_prefix = ""
transport = "filesystem"
root = "./root"
"#,
    );
    let service = spawn_service(dir.path(), &config, &[]).await;

    // Sanity: the source works for a legitimate path.
    let ok = send_request(service.addr, "GET", "/images/fs/ok.jpg/w320.webp").await;
    assert_eq!(ok.status, 200);

    for target in [
        "/images/fs/../sentinel.txt/w320.webp",
        "/images/fs/%2E%2E/sentinel.txt/w320.webp",
        "/images/fs/%2e%2e/sentinel.txt/w320.webp",
        "/images/fs/%252E%252E/sentinel.txt/w320.webp",
        "/images/fs/..%2Fsentinel.txt/w320.webp",
        "/images/fs/.%2E/sentinel.txt/w320.webp",
    ] {
        let response = send_request(service.addr, "GET", target).await;
        assert_eq!(response.status, 400, "target {target}");
        assert!(
            !String::from_utf8_lossy(&response.body).contains(sentinel),
            "sentinel bytes must never appear in a response, target {target}"
        );
    }
}

// ---------------------------------------------------------------------------
// Item 12: omitting v changes the cache policy, not the pixels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_omitting_v_returns_the_same_pixels_with_the_shorter_policy() {
    let fixture = Fixture::start(HashMap::from([(
        "/photos/pic.jpg".to_string(),
        Script::Bytes(ok_response(&jpeg_fixture(800, 400))),
    )]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = spawn_http_service(dir.path(), &fixture, ConfigOptions::default()).await;

    let versioned = send_request(
        service.addr,
        "GET",
        "/images/pub/photos/pic.jpg/w320.webp?v=abc123",
    )
    .await;
    let unversioned =
        send_request(service.addr, "GET", "/images/pub/photos/pic.jpg/w320.webp").await;

    assert_eq!(versioned.status, 200);
    assert_eq!(unversioned.status, 200);
    assert_eq!(
        versioned.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let unversioned_policy = unversioned.header("cache-control").unwrap();
    assert_eq!(
        unversioned_policy,
        format!("public, max-age={UNVERSIONED_TTL}")
    );
    assert!(!unversioned_policy.contains("immutable"));

    // Same Derived Image: compare decoded properties, not lossy bytes.
    let a = decode(&versioned.body).to_rgba8();
    let b = decode(&unversioned.body).to_rgba8();
    assert_eq!(a.dimensions(), b.dimensions());
    assert_eq!(a.dimensions(), (320, 160));
    for (x, y) in [(10, 10), (160, 80), (300, 150)] {
        let pa = a.get_pixel(x, y);
        let pb = b.get_pixel(x, y);
        for channel in 0..4 {
            assert!(
                pa[channel].abs_diff(pb[channel]) <= 2,
                "pixel ({x},{y}) channel {channel}: {pa:?} vs {pb:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Item 13: concurrency never exceeds max_concurrent_derivations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_concurrent_fixture_requests_never_exceed_max_derivations() {
    let jpeg = ok_response(&jpeg_fixture(64, 64));
    let mut routes = HashMap::new();
    for i in 0..8 {
        routes.insert(
            format!("/photos/p{i}.jpg"),
            Script::DelayedBytes {
                bytes: jpeg.clone(),
                delay_ms: 300,
            },
        );
    }
    let fixture = Fixture::start(routes).await;
    let dir = tempfile::tempdir().unwrap();
    let options = ConfigOptions {
        max_concurrent_derivations: 2,
        ..ConfigOptions::default()
    };
    let service = spawn_http_service(dir.path(), &fixture, options).await;

    let mut tasks = Vec::new();
    for i in 0..8 {
        let addr = service.addr;
        tasks.push(tokio::spawn(async move {
            send_request(
                addr,
                "GET",
                &format!("/images/pub/photos/p{i}.jpg/w320.webp"),
            )
            .await
        }));
    }
    for task in tasks {
        let response = task.await.unwrap();
        assert_eq!(response.status, 200);
    }

    assert_eq!(fixture.hits().len(), 8, "every request reached the fixture");
    let observed_max = fixture.inflight_max.load(Ordering::SeqCst);
    assert!(
        observed_max <= 2,
        "at most 2 fixture requests may be in flight, observed {observed_max}"
    );
}

// ---------------------------------------------------------------------------
// Item 14: the same source-path contract through a local S3-compatible Source
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_s3_source_serves_the_same_contract_and_key_excludes_v() {
    let bucket = "e2e-image-bucket";
    let jpeg = jpeg_fixture(800, 400);
    let mut object = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        jpeg.len()
    )
    .into_bytes();
    object.extend_from_slice(&jpeg);
    let fixture = Fixture::start(HashMap::from([(
        format!("/{bucket}/originals/photos/pic.jpg"),
        Script::Bytes(object),
    )]))
    .await;

    let dir = tempfile::tempdir().unwrap();
    let sources = format!(
        r#"[[sources]]
mount = "arch"
key_prefix = "originals"
transport = "s3"
bucket = "{bucket}"
region = "us-east-1"
endpoint_url = "http://127.0.0.1:{port}"
force_path_style = true
allow_private_destinations = true
"#,
        port = fixture.port
    );
    let config = config_toml(&ConfigOptions::default(), &sources);
    // Static credentials and IMDS opt-out are injected into the child only;
    // credentials are never accepted in TOML.
    let service = spawn_service(
        dir.path(),
        &config,
        &[
            ("AWS_ACCESS_KEY_ID", "e2e-access-key-id"),
            ("AWS_SECRET_ACCESS_KEY", "e2e-secret-access-key"),
            ("AWS_REGION", "us-east-1"),
            ("AWS_EC2_METADATA_DISABLED", "true"),
        ],
    )
    .await;

    let response = send_request(
        service.addr,
        "GET",
        "/images/arch/photos/pic.jpg/w320.webp?v=s3token99",
    )
    .await;
    assert_eq!(response.status, 200);
    assert!(is_webp(&response.body));
    assert_eq!(decode(&response.body).dimensions(), (320, 160));

    // The observed S3 key is Key Prefix + source path, and neither the path
    // nor the query carries the version token.
    let hits = fixture.hits();
    assert_eq!(hits.len(), 1);
    let (path, query) = match hits[0].split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (hits[0].as_str(), None),
    };
    assert_eq!(path, format!("/{bucket}/originals/photos/pic.jpg"));
    for param in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let name = param.split('=').next().unwrap_or(param);
        assert!(
            !name.eq_ignore_ascii_case("versionId") && name != "v",
            "S3 request must not carry a version token, got {:?}",
            hits[0]
        );
    }
    assert_no_line_contains(&service, "s3token99");
}

// ---------------------------------------------------------------------------
// Item 15: HTTPS Source with a local TLS fixture and its test CA
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_https_source_works_with_the_configured_test_ca() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "e2e test CA");
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone(), ca.der().clone()],
            rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tls_port = listener.local_addr().unwrap().port();
    let jpeg_body = ok_response(&jpeg_fixture(800, 400));
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let body = jpeg_body.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                // Read the request head, then always answer with the JPEG.
                let mut buf = [0u8; 2048];
                let mut head: Vec<u8> = Vec::new();
                loop {
                    let Ok(read) = tls.read(&mut buf).await else {
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
                let _ = tls.write_all(&body).await;
                let _ = tls.shutdown().await;
            });
        }
    });

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ca.pem"), ca.pem()).unwrap();
    let sources = format!(
        r#"[[sources]]
mount = "secure"
key_prefix = ""
transport = "http"
base_url = "https://localhost:{tls_port}"
ca_certificate_file = "ca.pem"
allow_private_destinations = true
"#
    );
    let config = config_toml(&ConfigOptions::default(), &sources);
    let service = spawn_service(dir.path(), &config, &[]).await;

    let response = send_request(
        service.addr,
        "GET",
        "/images/secure/photos/pic.jpg/w320.webp?v=tls1",
    )
    .await;
    assert_eq!(response.status, 200);
    assert!(is_webp(&response.body));
    assert_eq!(decode(&response.body).dimensions(), (320, 160));

    let events = wait_for_completions(&service, 1).await;
    assert_completion_shape(&events[0]);
    assert_eq!(events[0]["outcome"], "success");
}
