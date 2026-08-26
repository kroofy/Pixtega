//! HTTP application: composes request parsing, the Source registry, the
//! image processor, response headers, JSON error bodies, and completion
//! logging.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::response::Builder;
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;
use tokio::sync::Semaphore;

use crate::config::AppConfig;
use crate::errors::{Outcome, ProcessError, RequestError};
use crate::logging::CompletionEvent;
use crate::processor;
use crate::request::parse_request;
use crate::sources::SourceRegistry;
use crate::types::{OutputFormat, ResolvedRequest};

/// Shared state for the HTTP application.
pub struct AppState {
    pub config: AppConfig,
    pub registry: SourceRegistry,
    /// Process-wide derivation permits: no more than
    /// `max_concurrent_derivations` Source Objects are fetched or processed
    /// at once. Acquired before fetching, held through processing.
    pub derivation_permits: Semaphore,
}

impl AppState {
    pub fn new(config: AppConfig, registry: SourceRegistry) -> Arc<Self> {
        let permits = config.max_concurrent_derivations;
        Arc::new(AppState {
            config,
            registry,
            derivation_permits: Semaphore::new(permits),
        })
    }
}

/// Build the router serving the image contract.
///
/// One fallback handler owns every path: the URL grammar (prefix, Mount,
/// source path, Transform) belongs to [`parse_request`], not to the router.
pub fn build_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(state)
}

/// Bind `config.listen_address` and serve until shutdown (Ctrl-C).
///
/// libvips initialization and encoder verification happen in `main` before
/// this is called. Prints one `{"event":"listening",...}` JSON line to
/// standard output once the socket is bound, so a supervisor or test can
/// wait for readiness.
pub async fn run(config: AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    let registry = SourceRegistry::from_config(&config).await?;
    let listen_address = config.listen_address;
    let state = AppState::new(config, registry);
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind(listen_address).await?;
    let local_address = listener.local_addr()?;
    println!(
        "{}",
        serde_json::json!({
            "event": "listening",
            "address": local_address.to_string(),
        })
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Log fields gathered while answering one request. Only fields that were
/// actually established are set; the completion event omits the rest.
struct Report {
    outcome: Outcome,
    mount: Option<String>,
    width: Option<u32>,
    format: Option<OutputFormat>,
    upstream_status: Option<u16>,
    input_bytes: Option<u64>,
    output_bytes: Option<u64>,
}

impl Report {
    fn new(outcome: Outcome) -> Self {
        Report {
            outcome,
            mount: None,
            width: None,
            format: None,
            upstream_status: None,
            input_bytes: None,
            output_bytes: None,
        }
    }
}

/// The single request handler. Emits exactly one [`CompletionEvent`] per
/// request, covering the whole handler in `elapsed_ms`.
async fn handle(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let started = Instant::now();
    // Only the request line matters; the body (never read) is not Sync and
    // must not be captured across await points.
    let (parts, _body) = request.into_parts();
    let (response, report) = respond(&state, &parts.method, &parts.uri).await;
    let response = if parts.method == Method::HEAD {
        drop_body(response).await
    } else {
        response
    };

    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut event = CompletionEvent::new(response.status().as_u16(), report.outcome, elapsed_ms);
    event.mount = report.mount;
    event.width = report.width;
    event.format = report.format.map(OutputFormat::as_str);
    event.upstream_status = report.upstream_status;
    event.input_bytes = report.input_bytes;
    event.output_bytes = report.output_bytes;
    event.emit();

    response
}

async fn respond(state: &AppState, method: &Method, uri: &Uri) -> (Response, Report) {
    // HEAD takes exactly the GET path — same routing, derivation, status,
    // and headers. The body is dropped at the end of `handle`, so the
    // advertised Content-Length stays that of the equivalent GET response.
    if method != Method::GET && method != Method::HEAD {
        let response = error_response(state, StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        return (response, Report::new(Outcome::RejectedRequest));
    }

    // `parse_request` bounds the origin-form target (path + query). An
    // HTTP/1.1 absolute-form target additionally carries a scheme and
    // authority; bound the whole received target here so no request-target
    // form evades the 8192-byte limit.
    if uri.to_string().len() > crate::request::MAX_TARGET_BYTES {
        let err = RequestError::TargetTooLong;
        let response = error_response(state, taxonomy_status(err.status()), err.public_message());
        return (response, Report::new(err.outcome()));
    }

    // `Uri` exposes the raw, still-percent-encoded path and query.
    let resolved = match parse_request(&state.config, uri.path(), uri.query()) {
        Ok(resolved) => resolved,
        Err(err) => {
            let response =
                error_response(state, taxonomy_status(err.status()), err.public_message());
            return (response, Report::new(err.outcome()));
        }
    };

    let mut report = Report::new(Outcome::Success);
    report.mount = Some(resolved.mount.clone());
    report.width = Some(resolved.transform.width);
    report.format = Some(resolved.transform.format);

    // parse_request only accepts configured Mounts and the registry is built
    // from the same configuration, so a missing adapter is unreachable.
    let Some(source) = state.registry.get(&resolved.mount) else {
        report.outcome = Outcome::SourceUnavailable;
        let response = error_response(state, StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        return (response, report);
    };

    // One process-wide permit bounds fetching AND processing together; it is
    // held (`_permit` lives to the end of this scope) through both.
    let _permit = state
        .derivation_permits
        .acquire()
        .await
        .expect("derivation semaphore is never closed");

    let fetched = match source.fetch(&resolved.upstream_key).await {
        Ok(fetched) => fetched,
        Err(err) => {
            report.outcome = err.outcome();
            report.upstream_status = err.upstream_status();
            let response =
                error_response(state, taxonomy_status(err.status()), err.public_message());
            return (response, report);
        }
    };
    report.upstream_status = fetched.upstream_status;
    report.input_bytes = Some(fetched.bytes.len() as u64);

    // CPU-bound synchronous work on a blocking thread, permit still held.
    let transform = resolved.transform;
    let max_megapixels = state.config.max_source_megapixels;
    let source_bytes = fetched.bytes;
    let processed = tokio::task::spawn_blocking(move || {
        processor::process_image(&source_bytes, &transform, max_megapixels)
    })
    .await
    .unwrap_or_else(|join_error| {
        Err(ProcessError::Encode {
            detail: format!("processing task failed: {join_error}"),
        })
    });

    let derived = match processed {
        Ok(bytes) => bytes,
        Err(err) => {
            report.outcome = err.outcome();
            let response =
                error_response(state, taxonomy_status(err.status()), err.public_message());
            return (response, report);
        }
    };
    report.output_bytes = Some(derived.len() as u64);

    (success_response(state, &resolved, derived), report)
}

/// Replace the response body with an empty one, advertising the dropped
/// body's length through an explicit `Content-Length`. A HEAD response
/// thus carries exactly the headers of the equivalent GET — including a
/// truthful `Content-Length` — with no body on the wire.
async fn drop_body(response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("responses are built from in-memory bodies");
    parts
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    Response::from_parts(parts, Body::empty())
}

/// Every status produced by the error taxonomy is a valid HTTP status.
fn taxonomy_status(status: u16) -> StatusCode {
    StatusCode::from_u16(status).expect("error taxonomy produces valid HTTP statuses")
}

fn security_headers(builder: Builder) -> Builder {
    builder
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
        .header(header::X_FRAME_OPTIONS, "DENY")
}

fn success_response(state: &AppState, resolved: &ResolvedRequest, body: Vec<u8>) -> Response {
    let cache_control = if resolved.versioned {
        "public, max-age=31536000, immutable".to_string()
    } else {
        format!(
            "public, max-age={}",
            state.config.unversioned_success_ttl_seconds
        )
    };
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            resolved.transform.format.content_type(),
        )
        .header(header::CACHE_CONTROL, cache_control);
    security_headers(builder)
        .body(Body::from(body))
        .expect("static header set is always valid")
}

fn error_response(state: &AppState, status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "error": message }).to_string();
    let cache_control = if status == StatusCode::NOT_FOUND {
        format!("public, max-age={}", state.config.not_found_ttl_seconds)
    } else {
        "no-store".to_string()
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, cache_control);
    if status == StatusCode::METHOD_NOT_ALLOWED {
        builder = builder.header(header::ALLOW, "GET, HEAD");
    }
    security_headers(builder)
        .body(Body::from(body))
        .expect("static header set is always valid")
}
