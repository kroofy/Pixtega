//! S3 Source adapter contract tests.
//!
//! Uses a hand-rolled local S3-compatible fake: a raw HTTP/1.1 server on
//! 127.0.0.1 that understands path-style `GET /{bucket}/{key}` and answers
//! with S3 XML error documents or object bodies. Static test credentials are
//! injected through environment variables before any client is built, and
//! IMDS is disabled so the credential chain never touches a network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use image_service::config::S3SourceConfig;
use image_service::errors::SourceError;
use image_service::sources::s3::S3Source;
use image_service::sources::{FetchLimits, Source};
use image_service::types::UpstreamKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

static ENV_SETUP: Once = Once::new();

/// Tests in one binary share the process environment, so credentials are
/// set exactly once, before any SDK client is constructed.
fn setup_test_credentials() {
    ENV_SETUP.call_once(|| {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test-access-key-id");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test-secret-access-key");
        std::env::set_var("AWS_SESSION_TOKEN", "test-session-token");
        std::env::set_var("AWS_REGION", "us-east-1");
        // Keep the credential chain entirely off the network.
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
    });
}

// ---------------------------------------------------------------------------
// Fake S3 server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Recorded {
    path: String,
    query: Option<String>,
}

#[derive(Clone)]
enum Script {
    /// Write these raw bytes, then close.
    Bytes(Vec<u8>),
    /// Write these raw bytes, then hold the connection open silently.
    BytesThenStall(Vec<u8>),
    /// Read the request, never respond.
    Stall,
    /// 200 with chunked transfer encoding and no Content-Length.
    ChunkedHuge { total: usize },
}

struct FakeS3 {
    port: u16,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeS3 {
    async fn start(routes: HashMap<String, Script>) -> FakeS3 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
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
                            let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
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
                        None => {
                            let _ = stream
                                .write_all(&xml_error(404, "NoSuchKey", "unrouted test key"))
                                .await;
                        }
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        FakeS3 { port, requests }
    }

    fn endpoint(&self) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}", self.port)).unwrap()
    }

    fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Recorded> {
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
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target.to_string(), None),
    };
    Some(Recorded { path, query })
}

fn xml_error(status: u16, code: &str, message: &str) -> Vec<u8> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{message}</Message></Error>"
    );
    let reason = match status {
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body.as_bytes());
    response
}

fn object_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

// ---------------------------------------------------------------------------
// Adapter helpers
// ---------------------------------------------------------------------------

const BUCKET: &str = "test-image-bucket";

fn limits(max_bytes: u64, timeout: Duration) -> FetchLimits {
    FetchLimits {
        max_bytes,
        timeout,
        max_redirects: 3,
    }
}

async fn adapter(fake: &FakeS3, limits: FetchLimits) -> S3Source {
    setup_test_credentials();
    let config = S3SourceConfig {
        mount: "archive".to_string(),
        key_prefix_segments: Vec::new(),
        bucket: BUCKET.to_string(),
        region: "us-east-1".to_string(),
        endpoint_url: Some(fake.endpoint()),
        force_path_style: true,
    };
    S3Source::new(&config, limits)
        .await
        .expect("adapter construction")
}

fn key(segments: &[&str]) -> UpstreamKey {
    UpstreamKey::new(segments.iter().map(|s| s.to_string()).collect())
}

fn default_limits() -> FetchLimits {
    limits(1024, Duration::from_secs(10))
}

fn assert_no_version_in_query(recorded: &Recorded) {
    let params: Vec<&str> = recorded
        .query
        .as_deref()
        .map(|q| q.split('&').collect())
        .unwrap_or_default();
    for param in &params {
        let name = param.split('=').next().unwrap_or(param);
        assert!(
            !name.eq_ignore_ascii_case("versionId") && name != "v",
            "query must never carry a version token, got {:?}",
            recorded.query
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_observes_the_exact_key_without_any_version() {
    let path = format!("/{BUCKET}/originals/photos/cat.v1.jpg");
    let fake = FakeS3::start(HashMap::from([(
        path.clone(),
        Script::Bytes(object_response(b"object-bytes")),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let fetched = source
        .fetch(&key(&["originals", "photos", "cat.v1.jpg"]))
        .await
        .expect("fetch succeeds");
    assert_eq!(fetched.bytes, b"object-bytes");
    assert_eq!(fetched.upstream_status, Some(200));

    let requests = fake.recorded();
    assert_eq!(requests.len(), 1);
    // Path-style: bucket, then Key Prefix + source path joined with '/'.
    assert_eq!(requests[0].path, path);
    assert_no_version_in_query(&requests[0]);
}

#[tokio::test]
async fn unicode_key_segments_are_percent_encoded_on_the_wire() {
    // Routing is keyed on the exact encoded path, so a successful fetch
    // proves the encoding the fake observed.
    let path = format!("/{BUCKET}/media/ph%C3%B8to%20image.v1.jpg");
    let fake = FakeS3::start(HashMap::from([(
        path.clone(),
        Script::Bytes(object_response(b"unicode-object")),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let fetched = source
        .fetch(&key(&["media", "phøto image.v1.jpg"]))
        .await
        .expect("fetch succeeds");
    assert_eq!(fetched.bytes, b"unicode-object");
    assert_eq!(fake.recorded()[0].path, path);
}

#[tokio::test]
async fn no_such_key_maps_to_not_found() {
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/missing.jpg"),
        Script::Bytes(xml_error(
            404,
            "NoSuchKey",
            "The specified key does not exist.",
        )),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let err = source.fetch(&key(&["missing.jpg"])).await.unwrap_err();
    assert!(
        matches!(
            err,
            SourceError::NotFound {
                upstream_status: Some(404)
            }
        ),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn raw_404_with_unmodeled_code_maps_to_not_found() {
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/missing.jpg"),
        Script::Bytes(xml_error(404, "NotFound", "no such object")),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let err = source.fetch(&key(&["missing.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::NotFound { .. }),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn access_denied_maps_to_unavailable_and_never_not_found() {
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/denied.jpg"),
        Script::Bytes(xml_error(403, "AccessDenied", "Access Denied")),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let err = source.fetch(&key(&["denied.jpg"])).await.unwrap_err();
    assert!(
        !matches!(err, SourceError::NotFound { .. }),
        "denial must never be reported as absence, got {err:?}"
    );
    assert!(
        matches!(
            err,
            SourceError::Unavailable {
                upstream_status: Some(403),
                ..
            }
        ),
        "expected Unavailable with status 403, got {err:?}"
    );
}

#[tokio::test]
async fn other_service_errors_map_to_unavailable() {
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/broken.jpg"),
        Script::Bytes(xml_error(
            500,
            "InternalError",
            "We encountered an internal error.",
        )),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let err = source.fetch(&key(&["broken.jpg"])).await.unwrap_err();
    assert!(
        matches!(
            err,
            SourceError::Unavailable {
                upstream_status: Some(500),
                ..
            }
        ),
        "expected Unavailable with status 500, got {err:?}"
    );
}

#[tokio::test]
async fn advertised_content_length_over_limit_is_too_large_without_reading_the_body() {
    // Headers advertise an oversized object and the fake then stalls: only
    // the advertised-length check can return TooLarge this fast.
    let head =
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n";
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/huge.jpg"),
        Script::BytesThenStall(head.as_bytes().to_vec()),
    )]))
    .await;

    let source = adapter(&fake, limits(1024, Duration::from_secs(10))).await;
    let started = Instant::now();
    let err = source.fetch(&key(&["huge.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::TooLarge { .. }),
        "expected TooLarge, got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "TooLarge must come from the advertised length, not from body streaming"
    );
}

#[tokio::test]
async fn streamed_body_over_limit_without_content_length_is_too_large() {
    // Chunked body without Content-Length: only the streamed check applies.
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/chunky.jpg"),
        Script::ChunkedHuge { total: 256 * 1024 },
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let err = source.fetch(&key(&["chunky.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::TooLarge { .. }),
        "expected TooLarge, got {err:?}"
    );
}

#[tokio::test]
async fn body_exactly_at_the_limit_is_fetched() {
    let body = vec![b'z'; 1024];
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/fits.jpg"),
        Script::Bytes(object_response(&body)),
    )]))
    .await;

    let source = adapter(&fake, default_limits()).await;
    let fetched = source.fetch(&key(&["fits.jpg"])).await.unwrap();
    assert_eq!(fetched.bytes.len(), 1024);
}

#[tokio::test]
async fn stalling_server_maps_to_timeout() {
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/slow.jpg"),
        Script::Stall,
    )]))
    .await;

    let source = adapter(&fake, limits(1024, Duration::from_millis(300))).await;
    let started = Instant::now();
    let err = source.fetch(&key(&["slow.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Timeout),
        "expected Timeout, got {err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn body_that_stalls_mid_stream_maps_to_timeout() {
    let mut head =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 800\r\nConnection: close\r\n\r\n"
            .to_vec();
    head.extend_from_slice(&[b'p'; 100]);
    let fake = FakeS3::start(HashMap::from([(
        format!("/{BUCKET}/stall.jpg"),
        Script::BytesThenStall(head),
    )]))
    .await;

    let source = adapter(&fake, limits(1024, Duration::from_millis(300))).await;
    let err = source.fetch(&key(&["stall.jpg"])).await.unwrap_err();
    assert!(
        matches!(err, SourceError::Timeout),
        "expected Timeout, got {err:?}"
    );
}
