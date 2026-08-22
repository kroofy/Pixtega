//! HTTP(S) Source adapter contract tests.
//!
//! Uses hand-rolled fixture servers on 127.0.0.1 (raw HTTP/1.1 over tokio
//! TcpListener) for full control over status lines, Content-Length lies,
//! transfer encodings, stalls, and redirect targets. The TLS fixture uses an
//! rcgen-generated CA + leaf served through tokio-rustls.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pixtega::config::HttpSourceConfig;
use pixtega::errors::SourceError;
use pixtega::sources::http::HttpSource;
use pixtega::sources::{FetchLimits, Source};
use pixtega::types::UpstreamKey;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

// ---------------------------------------------------------------------------
// Fixture server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Recorded {
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// What the fixture writes back for one route (responses always close the
/// connection afterwards unless they stall, so each hop is one connection).
#[derive(Clone)]
enum Script {
    /// Write these raw bytes, then close.
    Bytes(Vec<u8>),
    /// Write these raw bytes, then hold the connection open without further
    /// data (for header-only and mid-body stall tests).
    BytesThenStall(Vec<u8>),
    /// Read the request, never write anything, hold the connection open.
    Stall,
    /// Chunked transfer encoding without Content-Length, `total` body bytes.
    ChunkedHuge { total: usize },
    /// No Content-Length, no chunking: body delimited by connection close.
    CloseDelimited { total: usize },
}

struct Fixture {
    port: u16,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Fixture {
    async fn start(routes: HashMap<String, Script>) -> Fixture {
        Fixture::start_with(|_| routes).await
    }

    /// Like `start`, but the routes may embed the fixture's own port.
    async fn start_with(routes: impl FnOnce(u16) -> HashMap<String, Script>) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes = routes(port);
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                let routes = routes.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut stream).await else {
                        return;
                    };
                    let script = routes.get(&request.path).cloned();
                    recorded.lock().unwrap().push(request);
                    match script {
                        Some(Script::Bytes(bytes)) => {
                            let _ = stream.write_all(&bytes).await;
                        }
                        Some(Script::BytesThenStall(bytes)) => {
                            let _ = stream.write_all(&bytes).await;
                            tokio::time::sleep(Duration::from_secs(60)).await;
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
                                    return;
                                }
                                sent += n;
                            }
                            let _ = stream.write_all(b"0\r\n\r\n").await;
                        }
                        Some(Script::CloseDelimited { total }) => {
                            let head = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(head).await;
                            let chunk = vec![b'y'; 8192];
                            let mut sent = 0usize;
                            while sent < total {
                                let n = chunk.len().min(total - sent);
                                if stream.write_all(&chunk[..n]).await.is_err() {
                                    return;
                                }
                                sent += n;
                            }
                        }
                        None => {
                            let _ = stream
                                .write_all(status_response(404, "Not Found").as_bytes())
                                .await;
                        }
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        Fixture { port, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

/// Read one request head (through `\r\n\r\n`) and parse the target line and
/// headers. GET requests carry no body.
async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> Option<Recorded> {
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
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target.to_string(), None),
    };
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            line.split_once(':')
                .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    Some(Recorded {
        path,
        query,
        headers,
    })
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

fn status_response(code: u16, reason: &str) -> String {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

fn redirect_response(code: u16, location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {code} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------
// Adapter helpers
// ---------------------------------------------------------------------------

fn limits(max_bytes: u64, timeout: Duration, max_redirects: u32) -> FetchLimits {
    FetchLimits {
        max_bytes,
        timeout,
        max_redirects,
    }
}

fn adapter(base_url: &str, ca: Option<PathBuf>, limits: FetchLimits) -> HttpSource {
    let config = HttpSourceConfig {
        mount: "public".to_string(),
        key_prefix_segments: Vec::new(),
        base_url: Url::parse(base_url).unwrap(),
        ca_certificate_file: ca,
    };
    HttpSource::new(&config, limits).expect("adapter construction")
}

fn key(segments: &[&str]) -> UpstreamKey {
    UpstreamKey::new(segments.iter().map(|s| s.to_string()).collect())
}

fn default_limits() -> FetchLimits {
    limits(1024, Duration::from_secs(10), 3)
}

// ---------------------------------------------------------------------------
// Success and URL construction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_returns_bytes_and_builds_percent_encoded_path() {
    let encoded_path = "/base/media/dir.v2/ph%C3%B8to%20image.jpg";
    let fixture = Fixture::start(HashMap::from([(
        encoded_path.to_string(),
        Script::Bytes(ok_response(b"image-bytes")),
    )]))
    .await;

    let source = adapter(&fixture.url("/base"), None, default_limits());
    let fetched = source
        .fetch(&key(&["media", "dir.v2", "phøto image.jpg"]))
        .await
        .expect("fetch succeeds");

    assert_eq!(fetched.bytes, b"image-bytes");
    assert_eq!(fetched.upstream_status, Some(200));

    let requests = fixture.recorded();
    assert_eq!(requests.len(), 1);
    // Exact upstream target: base path + percent-encoded decoded segments,
    // no query string, and in particular no `v` anywhere.
    assert_eq!(requests[0].path, encoded_path);
    assert_eq!(requests[0].query, None);
    assert_eq!(requests[0].header("accept-encoding"), Some("identity"));
}

#[tokio::test]
async fn base_url_with_trailing_slash_builds_the_same_path() {
    let fixture = Fixture::start(HashMap::from([(
        "/base/img.jpg".to_string(),
        Script::Bytes(ok_response(b"ok")),
    )]))
    .await;

    let source = adapter(
        &format!("{}/", fixture.url("/base")),
        None,
        default_limits(),
    );
    source
        .fetch(&key(&["img.jpg"]))
        .await
        .expect("fetch succeeds");
    assert_eq!(fixture.recorded()[0].path, "/base/img.jpg");
}

// ---------------------------------------------------------------------------
// Status mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn statuses_404_and_410_map_to_not_found() {
    for code in [404u16, 410u16] {
        let fixture = Fixture::start(HashMap::from([(
            "/img.jpg".to_string(),
            Script::Bytes(status_response(code, "Gone-ish").into_bytes()),
        )]))
        .await;
        let source = adapter(&fixture.url(""), None, default_limits());
        let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
        assert!(
            matches!(err, SourceError::NotFound { upstream_status: Some(s) } if s == code),
            "status {code}: expected NotFound, got {err:?}"
        );
    }
}

#[tokio::test]
async fn statuses_403_500_503_map_to_unavailable() {
    for code in [403u16, 500u16, 503u16] {
        let fixture = Fixture::start(HashMap::from([(
            "/img.jpg".to_string(),
            Script::Bytes(status_response(code, "Nope").into_bytes()),
        )]))
        .await;
        let source = adapter(&fixture.url(""), None, default_limits());
        let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
        assert!(
            matches!(err, SourceError::Unavailable { upstream_status: Some(s), .. } if s == code),
            "status {code}: expected Unavailable, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redirect_chain_within_base_path_is_followed_up_to_the_limit() {
    let fixture = Fixture::start(HashMap::from([
        (
            "/base/r0".to_string(),
            Script::Bytes(redirect_response(301, "/base/r1")),
        ),
        (
            "/base/r1".to_string(),
            // Relative Location, resolved against the current URL.
            Script::Bytes(redirect_response(303, "r2")),
        ),
        (
            "/base/r2".to_string(),
            Script::Bytes(redirect_response(307, "/base/final.jpg")),
        ),
        (
            "/base/final.jpg".to_string(),
            Script::Bytes(ok_response(b"arrived")),
        ),
    ]))
    .await;

    let source = adapter(
        &fixture.url("/base"),
        None,
        limits(1024, Duration::from_secs(10), 3),
    );
    let fetched = source.fetch(&key(&["r0"])).await.expect("chain followed");
    assert_eq!(fetched.bytes, b"arrived");

    let paths: Vec<String> = fixture.recorded().iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        paths,
        ["/base/r0", "/base/r1", "/base/r2", "/base/final.jpg"]
    );
}

#[tokio::test]
async fn redirect_chain_exceeding_the_limit_is_unavailable() {
    let mut routes = HashMap::new();
    for hop in 0..5 {
        routes.insert(
            format!("/base/r{hop}"),
            Script::Bytes(redirect_response(302, &format!("/base/r{}", hop + 1))),
        );
    }
    let fixture = Fixture::start(routes).await;

    let source = adapter(
        &fixture.url("/base"),
        None,
        limits(1024, Duration::from_secs(10), 3),
    );
    let err = source.fetch(&key(&["r0"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "expected Unavailable, got {err:?}"
    );
    // Initial request + exactly max_redirects follows; the 4th redirect
    // response is not followed.
    assert_eq!(fixture.recorded().len(), 4);
}

#[tokio::test]
async fn redirect_to_a_different_port_host_or_scheme_is_unavailable() {
    let target_builders: Vec<fn(u16) -> String> = vec![
        // Different effective port (nothing listens there; the adapter must
        // reject before ever connecting).
        |port| format!("http://127.0.0.1:{}/base/x", port.wrapping_add(1)),
        // Different host name, same address and port.
        |port| format!("http://localhost:{port}/base/x"),
        // Different scheme, same host and port.
        |port| format!("https://127.0.0.1:{port}/base/x"),
    ];
    for build_target in target_builders {
        let mut built = String::new();
        let fixture = Fixture::start_with(|port| {
            built = build_target(port);
            HashMap::from([
                (
                    "/base/img.jpg".to_string(),
                    Script::Bytes(redirect_response(302, &built)),
                ),
                // If the adapter wrongly followed, this would answer 200.
                ("/base/x".to_string(), Script::Bytes(ok_response(b"no"))),
            ])
        })
        .await;
        let source = adapter(&fixture.url("/base"), None, default_limits());
        let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
        assert!(
            matches!(err, SourceError::Unavailable { .. }),
            "target {built}: expected Unavailable, got {err:?}"
        );
        // The rejected target was never contacted.
        assert_eq!(fixture.recorded().len(), 1, "target {built}");
    }
}

#[tokio::test]
async fn redirect_escaping_the_base_path_is_unavailable() {
    for location in ["/outside/x", "/baseother/x", "/base/../outside/x", "/"] {
        let fixture = Fixture::start(HashMap::from([
            (
                "/base/img.jpg".to_string(),
                Script::Bytes(redirect_response(302, location)),
            ),
            // If the adapter wrongly followed, these would answer 200.
            ("/outside/x".to_string(), Script::Bytes(ok_response(b"no"))),
            (
                "/baseother/x".to_string(),
                Script::Bytes(ok_response(b"no")),
            ),
            ("/".to_string(), Script::Bytes(ok_response(b"no"))),
        ]))
        .await;
        let source = adapter(&fixture.url("/base"), None, default_limits());
        let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
        assert!(
            matches!(err, SourceError::Unavailable { .. }),
            "location {location}: expected Unavailable, got {err:?}"
        );
        assert_eq!(fixture.recorded().len(), 1, "location {location}");
    }
}

#[tokio::test]
async fn redirect_staying_beneath_a_deep_base_path_is_followed() {
    let fixture = Fixture::start(HashMap::from([
        (
            "/base/img.jpg".to_string(),
            Script::Bytes(redirect_response(308, "/base/deeper/img.jpg")),
        ),
        (
            "/base/deeper/img.jpg".to_string(),
            Script::Bytes(ok_response(b"deep")),
        ),
    ]))
    .await;
    let source = adapter(&fixture.url("/base"), None, default_limits());
    let fetched = source.fetch(&key(&["img.jpg"])).await.unwrap();
    assert_eq!(fetched.bytes, b"deep");
}

#[tokio::test]
async fn redirect_without_location_is_unavailable() {
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::Bytes(status_response(302, "Found").into_bytes()),
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Content encoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn content_encoding_other_than_identity_is_unavailable() {
    for encoding in ["gzip", "br", "deflate"] {
        let body = b"compressed-ish";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: {encoding}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut bytes = raw.into_bytes();
        bytes.extend_from_slice(body);
        let fixture = Fixture::start(HashMap::from([(
            "/img.jpg".to_string(),
            Script::Bytes(bytes),
        )]))
        .await;
        let source = adapter(&fixture.url(""), None, default_limits());
        let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
        assert!(
            matches!(err, SourceError::Unavailable { .. }),
            "encoding {encoding}: expected Unavailable, got {err:?}"
        );
    }
}

/// A connection that dies mid-body (fewer bytes than Content-Length, then
/// close) is unavailability, reported with the body-read detail — the terse
/// mapping distinguishes body failures from connect/request failures.
#[tokio::test]
async fn body_shorter_than_content_length_is_an_unavailable_body_read_failure() {
    let raw = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
    let mut bytes = raw.as_bytes().to_vec();
    bytes.extend_from_slice(b"only-ten-b");
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::Bytes(bytes),
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    match err {
        SourceError::Unavailable { detail, .. } => {
            assert_eq!(detail, "upstream body read failed");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn explicit_identity_content_encoding_is_accepted() {
    let body = b"plain";
    let raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut bytes = raw.into_bytes();
    bytes.extend_from_slice(body);
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::Bytes(bytes),
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let fetched = source.fetch(&key(&["img.jpg"])).await.unwrap();
    assert_eq!(fetched.bytes, body);
}

// ---------------------------------------------------------------------------
// Byte limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn advertised_content_length_over_limit_is_too_large_without_reading_the_body() {
    // The fixture sends only the header block and then stalls: if the
    // adapter tried to read the body it would run into the (generous)
    // timeout instead of returning TooLarge promptly.
    let head = "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n";
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::BytesThenStall(head.as_bytes().to_vec()),
    )]))
    .await;

    let source = adapter(
        &fixture.url(""),
        None,
        limits(1024, Duration::from_secs(10), 3),
    );
    let started = Instant::now();
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(
            err,
            SourceError::TooLarge {
                upstream_status: Some(200)
            }
        ),
        "expected TooLarge, got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "TooLarge must be decided from the header, not by waiting on the body"
    );
}

#[tokio::test]
async fn streamed_body_without_content_length_over_limit_is_too_large() {
    // Close-delimited body, no Content-Length header at all: only the
    // streamed check can catch this.
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::CloseDelimited { total: 64 * 1024 },
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::TooLarge { .. }),
        "expected TooLarge, got {err:?}"
    );
}

#[tokio::test]
async fn huge_chunked_body_without_content_length_is_too_large() {
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::ChunkedHuge { total: 256 * 1024 },
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::TooLarge { .. }),
        "expected TooLarge, got {err:?}"
    );
}

#[tokio::test]
async fn body_exactly_at_the_limit_is_fetched() {
    let body = vec![b'z'; 1024];
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::Bytes(ok_response(&body)),
    )]))
    .await;
    let source = adapter(&fixture.url(""), None, default_limits());
    let fetched = source.fetch(&key(&["img.jpg"])).await.unwrap();
    assert_eq!(fetched.bytes.len(), 1024);
}

// ---------------------------------------------------------------------------
// Timeouts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_that_never_responds_maps_to_timeout() {
    let fixture = Fixture::start(HashMap::from([("/img.jpg".to_string(), Script::Stall)])).await;
    let source = adapter(
        &fixture.url(""),
        None,
        limits(1024, Duration::from_millis(300), 3),
    );
    let started = Instant::now();
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Timeout),
        "expected Timeout, got {err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn body_that_stalls_mid_stream_maps_to_timeout() {
    // Headers plus a partial body, then silence: the overall timeout must
    // cover body streaming, not just the response head.
    let mut head = b"HTTP/1.1 200 OK\r\nContent-Length: 800\r\nConnection: close\r\n\r\n".to_vec();
    head.extend_from_slice(&[b'p'; 100]);
    let fixture = Fixture::start(HashMap::from([(
        "/img.jpg".to_string(),
        Script::BytesThenStall(head),
    )]))
    .await;
    let source = adapter(
        &fixture.url(""),
        None,
        limits(1024, Duration::from_millis(300), 3),
    );
    let started = Instant::now();
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Timeout),
        "expected Timeout, got {err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---------------------------------------------------------------------------
// HTTPS / TLS
// ---------------------------------------------------------------------------

struct TlsFixture {
    port: u16,
    ca_file: tempfile::NamedTempFile,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

/// TLS fixture server: rcgen CA + leaf for SAN "localhost" only (no IP SAN),
/// served with tokio-rustls. Answers every request with a fixed 200 body.
async fn start_tls_fixture(body: &'static [u8]) -> TlsFixture {
    // Both `ring` and `aws-lc-rs` providers are enabled through feature
    // unification, so a process default must be installed explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "adapter test CA");
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

    let mut ca_file = tempfile::NamedTempFile::new().unwrap();
    ca_file.write_all(ca.pem().as_bytes()).unwrap();
    ca_file.flush().unwrap();

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone(), ca.der().clone()],
            rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                // Handshake failures (unknown CA, hostname mismatch on the
                // client side) simply drop the connection.
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let Some(request) = read_request(&mut tls).await else {
                    return;
                };
                recorded.lock().unwrap().push(request);
                let _ = write_all(&mut tls, &ok_response(body)).await;
                let _ = tls.shutdown().await;
            });
        }
    });

    TlsFixture {
        port,
        ca_file,
        requests,
    }
}

async fn write_all(stream: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> std::io::Result<()> {
    stream.write_all(bytes).await
}

#[tokio::test]
async fn https_succeeds_with_the_configured_test_ca() {
    let fixture = start_tls_fixture(b"tls-bytes").await;
    let source = adapter(
        &format!("https://localhost:{}", fixture.port),
        Some(fixture.ca_file.path().to_path_buf()),
        default_limits(),
    );
    let fetched = source
        .fetch(&key(&["secure", "img.jpg"]))
        .await
        .expect("HTTPS fetch with test CA succeeds");
    assert_eq!(fetched.bytes, b"tls-bytes");
    assert_eq!(fixture.requests.lock().unwrap()[0].path, "/secure/img.jpg");
}

#[tokio::test]
async fn https_without_the_test_ca_is_unavailable() {
    let fixture = start_tls_fixture(b"tls-bytes").await;
    let source = adapter(
        &format!("https://localhost:{}", fixture.port),
        None,
        default_limits(),
    );
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "untrusted server certificate must be Unavailable, got {err:?}"
    );
}

#[tokio::test]
async fn https_hostname_verification_is_enforced_with_the_test_ca() {
    // The leaf is valid for "localhost" only; connecting by IP must fail
    // even though the CA itself is trusted.
    let fixture = start_tls_fixture(b"tls-bytes").await;
    let source = adapter(
        &format!("https://127.0.0.1:{}", fixture.port),
        Some(fixture.ca_file.path().to_path_buf()),
        default_limits(),
    );
    let err = source.fetch(&key(&["img.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Unavailable { .. }),
        "hostname mismatch must be Unavailable, got {err:?}"
    );
    assert!(
        fixture.requests.lock().unwrap().is_empty(),
        "no request may complete over a mis-verified connection"
    );
}

#[tokio::test]
async fn unreadable_ca_certificate_file_is_a_config_error() {
    let config = HttpSourceConfig {
        mount: "public".to_string(),
        key_prefix_segments: Vec::new(),
        base_url: Url::parse("https://localhost:1").unwrap(),
        ca_certificate_file: Some(PathBuf::from("/nonexistent/ca.pem")),
    };
    assert!(HttpSource::new(&config, default_limits()).is_err());
}
