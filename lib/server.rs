use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::daylight_generated::daylight::common::{self};
use crate::daylight_generated::daylight::html;
use crate::errors::{FatalError, NonFatalError};
use crate::languages;
use crate::processors::{HtmlProcessor, Processor, SpansProcessor};

use axum::{
    body::Bytes,
    extract,
    routing::{get, post},
    Router,
};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use http::Request;
use tokio::time::Duration;
use tower_http::request_id::RequestId;
use tracing::instrument;

const MAX_REQUEST_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
const MAX_FILE_SIZE: usize = 256 * 1024 * 1024; // 256MB

/// Application state.
#[derive(Clone)]
pub struct Server {
    pub default_per_file_timeout: Duration,
    pub max_per_file_timeout: Duration,
}

/// Try slicing out contents of a file from a request body, without making copies.
#[instrument(err, skip(file, body, language))]
fn prepare_file_contents(
    file: &common::File<'_>,
    body: Bytes,
    filename: Arc<str>,
    // Sent by reference to avoid writing Result<(Bytes, Language), (NonFatalError, Language)>.
    language: &mut Option<languages::SharedConfig>,
) -> Result<Bytes, NonFatalError> {
    *language = if file.language() == common::Language::Unspecified {
        languages::from_path(std::path::Path::new(filename.as_ref()))
    } else {
        file.language().try_into().ok()
    };

    if language.is_none() {
        Err(NonFatalError::InvalidLanguage)?
    } else if file.contents().is_none_or(|s| s.is_empty()) {
        Err(NonFatalError::EmptyFile)?
    } else if file.contents().unwrap().bytes().len() > MAX_FILE_SIZE {
        Err(NonFatalError::FileTooLarge)?
    }

    let slice = file.contents().unwrap().bytes();
    let offset = slice.as_ptr() as usize - body.as_ptr() as usize;
    let contents = body.slice(offset..offset + slice.len());
    Ok(contents)
}

/// Generic handler that processes files using a specific Processor implementation.
#[instrument(err, skip(state, body), fields(num_files, timeout_ms, request_size = body.len()))]
pub async fn generic_handler<P: Processor>(
    extract::State(state): extract::State<Server>,
    body: Bytes,
) -> Result<axum::response::Response, FatalError> {
    // Prepare this request.
    let request = flatbuffers::root::<html::Request>(&body)?;
    let timeout_ms = request.timeout_ms();
    let timeout = if timeout_ms == 0 {
        state.default_per_file_timeout
    } else {
        Duration::from_millis(timeout_ms)
    };
    if timeout > state.max_per_file_timeout {
        Err(FatalError::TimeoutTooLarge(state.max_per_file_timeout))?
    }
    let timeout_flag: Arc<AtomicUsize> = Arc::default();
    let files = request.files().unwrap_or_default();
    tracing::Span::current().record("num_files", files.len());
    tracing::Span::current().record("timeout_ms", timeout_ms);
    if files.is_empty() {
        return P::build_response(vec![]);
    }

    // This is the heart of the app: efficiently enqueuing concurrent highlighting requests,
    // propagating cancellation signals, and returning them in a stream, without
    // starving the tokio event loop and while processing as many documents as possible.
    let tasks = files
        .iter()
        .map(|file| {
            let ident = file.ident();
            let filename: Arc<str> = file.filename().unwrap_or_default().into();
            let body = body.clone(); // not a full memory copy, Bytes has zero-cost clone()
            let timeout_flag = timeout_flag.clone();
            let include_injections = file.include_injections();

            async move {
                let mut language_ptr: Option<languages::SharedConfig> = None;
                let contents =
                    match prepare_file_contents(&file, body, filename.clone(), &mut language_ptr) {
                        Ok(ok) => ok,
                        Err(reason) => {
                            return crate::processors::Outcome::failure(ident, filename, language_ptr, reason);
                        }
                    };
                let Some(language) = language_ptr else {
                    return crate::processors::Outcome::failure(ident, filename, None, NonFatalError::InvalidLanguage);
                };

                // Clones are needed for error handling paths (but are cheap, because these are Arcs).
                let cancellation_flag = timeout_flag.clone();
                let cancellation_flag_for_timeout = cancellation_flag.clone();
                let filename_for_join_error = filename.clone();
                let filename_for_timeout = filename.clone();

                // Spawn a blocking task for highlighting this file
                let task = tokio::task::spawn_blocking(move || {
                    P::process(
                        ident,
                        filename,
                        language,
                        contents,
                        include_injections,
                        cancellation_flag,
                    )
                })
                .map(move |t| {
                    // Thread-join errors are unlikely but possible
                    t.map_err(NonFatalError::from).unwrap_or_else(|reason| {
                        tracing::warn!("Join error encountered, this is upsetting: {reason}");
                        crate::processors::Outcome::failure(ident, filename_for_join_error, Some(language), reason)
                    })
                });

                // Run the task with the specified timeout
                tokio::time::timeout(timeout, task)
                    .await
                    .unwrap_or_else(|_elapsed| {
                        // Timeout occurred - set the cancellation flag so inflight tree-sitter-side tasks
                        // know that they should cancel and return.
                        cancellation_flag_for_timeout.store(1, Ordering::SeqCst);
                        crate::processors::Outcome::failure(ident, filename_for_timeout, language_ptr, NonFatalError::TimedOut)
                    })
            }
        })
        .collect::<FuturesUnordered<_>>();
    // Wait on all in-flight tasks simultaneously with .collect() and build a response.
    P::build_response(tasks.collect().await)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        _ = ctrl_c => { tracing::info!("Received SIGINT (Ctrl+C), starting graceful shutdown") },
        _ = terminate => { tracing::info!("Received SIGTERM, starting graceful shutdown") },
    }
}

// Public interface follows.

/// Build a router for a Daylight application.
pub fn router(default_per_file_timeout: Duration, max_per_file_timeout: Duration) -> Router {
    let state = Server {
        default_per_file_timeout,
        max_per_file_timeout,
    };
    // use axum_tracing_opentelemetry::middleware;
    use tower_http::*;

    let counter = metrics::in_flight_requests::InFlightRequestsCounter::new();
    let layer = tower::ServiceBuilder::new()
        .layer(catch_panic::CatchPanicLayer::new())
        .layer(compression::CompressionLayer::new()) // Request ID must come before tracing to be available in spans
        .layer(decompression::DecompressionLayer::new())
        .layer(request_id::SetRequestIdLayer::x_request_id(
            request_id::MakeRequestUuid,
        ))
        .layer(request_id::PropagateRequestIdLayer::x_request_id())
        .layer(
            trace::TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let request_id = request
                    .extensions()
                    .get::<RequestId>()
                    .and_then(|id| id.header_value().to_str().ok())
                    .unwrap_or("unknown");

                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                    request_id = %request_id,
                )
            }),
        )
        .layer(metrics::InFlightRequestsLayer::new(counter))
        // .layer(middleware::OtelInResponseLayer::default())
        // .layer(middleware::OtelAxumLayer::default())
        .layer(extract::DefaultBodyLimit::max(MAX_REQUEST_SIZE));

    Router::new()
        .route("/v1/html", post(generic_handler::<HtmlProcessor>))
        .route("/v1/spans", post(generic_handler::<SpansProcessor>))
        .route("/health", get("ok"))
        .layer(layer)
        .with_state(state)
}

/// Run a Daylight application.
pub async fn run(
    port: u16,
    default_per_file_timeout: Duration,
    max_per_file_timeout: Duration,
) -> anyhow::Result<()> {
    let app = router(default_per_file_timeout, max_per_file_timeout);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    tracing::info!("Listening on localhost:{}", port);

    // Graceful shutdown handler
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Server shutdown complete");

    Ok(())
}
