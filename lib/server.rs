use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::daylight_generated::daylight::common::{self};
use crate::daylight_generated::daylight::html;
use crate::languages;
use axum::{
    body::Bytes,
    extract,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use http::{Request, StatusCode};
use opentelemetry::trace;
use thiserror::Error;
use tokio::time::Duration;
use tower_http::request_id::RequestId;
use tracing::instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tree_sitter_highlight as ts;

// FlatBuffers maximum size is 2GB (2^31 - 1 bytes)
const MAX_REQUEST_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB

// Maximum size per individual file
const MAX_FILE_SIZE: usize = 256 * 1024 * 1024; // 256MB

#[derive(Clone)]
pub struct AppState {
    pub default_per_file_timeout: Duration,
    pub max_per_file_timeout: Duration,
}

#[derive(Default)]
struct ThreadState {
    highlighter: ts::Highlighter,
    renderer: ts::HtmlRenderer,
}

thread_local! {
    pub static PER_THREAD: RefCell<ThreadState> = RefCell::default();
}

#[derive(Debug, Error)]
pub enum FatalError {
    #[error("Decoding request failed")]
    DecodeError(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("Timeout too large (max supported: {max}ms)", max = .0.as_millis())]
    TimeoutTooLarge(Duration),
}

#[derive(Clone, Copy, Debug)]
pub enum NonFatalError {
    Cancelled,
    EmptyFile,
    FileTooLarge,
    InvalidLanguage,
    ThreadError,
    TimedOut,
    UnknownError,
}

impl From<ts::Error> for NonFatalError {
    fn from(value: ts::Error) -> Self {
        match value {
            ts::Error::Cancelled => Self::TimedOut,
            ts::Error::InvalidLanguage => Self::InvalidLanguage,
            ts::Error::Unknown => Self::UnknownError,
        }
    }
}

impl Into<common::ErrorCode> for NonFatalError {
    fn into(self) -> common::ErrorCode {
        match self {
            Self::TimedOut => common::ErrorCode::TimedOut,
            Self::Cancelled => common::ErrorCode::TimedOut,
            Self::InvalidLanguage => common::ErrorCode::UnknownLanguage,
            Self::FileTooLarge => common::ErrorCode::FileTooLarge,
            Self::ThreadError => common::ErrorCode::UnknownError,
            Self::UnknownError => common::ErrorCode::UnknownError,
            Self::EmptyFile => common::ErrorCode::NoError,
        }
    }
}

impl IntoResponse for FatalError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}

#[instrument(skip(doc_results), fields(count = doc_results.len()))]
fn build_response(doc_results: Vec<HighlightOutput>) -> Result<axum::response::Response, FatalError> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);

    // Build documents
    let documents: Vec<_> = doc_results
        .into_iter()
        .map(|doc| {
            let filename = builder.create_string(doc.filename());

            let lines = match doc {
                HighlightOutput::Success { ref lines, .. } =>
                    lines.iter().map(|line| builder.create_string(line)).collect(),
                _ => vec![],
            };

            let lines_vec = builder.create_vector(&lines);

            html::Document::create(
                &mut builder,
                &html::DocumentArgs {
                    ident: doc.ident(),
                    filename: Some(filename),
                    language: doc.fb_language(),
                    lines: Some(lines_vec),
                    error_code: doc.error_code(),
                },
            )
        })
        .collect();

    let documents_vec = builder.create_vector(&documents);

    // Build response
    let fb_response = html::Response::create(
        &mut builder,
        &html::ResponseArgs {
            documents: Some(documents_vec),
        },
    );

    builder.finish(fb_response, None);
    let response_bytes = builder.finished_data();

    // Use Bytes::copy_from_slice to create a response without extra allocation
    Ok((StatusCode::OK, Bytes::copy_from_slice(response_bytes)).into_response())
}

pub enum HighlightOutput {
    Success {
        ident: u16,
        filename: Arc<str>,
        language: languages::SharedConfig,
        lines: Vec<String>,
    },
    Failure {
        ident: u16,
        filename: Arc<str>,
        language: Option<languages::SharedConfig>,
        reason: NonFatalError,
    }

}

impl HighlightOutput {
    fn ident(&self) -> u16 {
        match self {
            Self::Success { ident, .. } => *ident,
            Self::Failure { ident, .. } => *ident,
        }
    }

    fn filename<'a>(&'a self) -> &'a str {
        match self {
            Self::Success { filename, .. } => filename.as_ref(),
            Self::Failure { .. } => Default::default(),
        }
    }

    fn fb_language(&self) -> common::Language {
        match self {
            Self::Success { language, ..} => language.fb_language,
            Self::Failure { language, .. } => language.map(|l| l.fb_language).unwrap_or_default(),
        }
    }

    fn error_code(&self) -> common::ErrorCode {
        match self {
            Self::Success { ..} => common::ErrorCode::NoError,
            Self::Failure { reason, .. } => (*reason).into(),
        }
    }


}

fn callback(highlight: ts::Highlight, output: &mut Vec<u8>) {
    let kind = languages::ALL_HIGHLIGHT_NAMES[highlight.0];
    output.extend_from_slice(b"class=\"");
    output.extend_from_slice(kind.as_bytes());
    output.extend_from_slice(b"\"");
}

#[instrument(skip(language, contents, cancellation_flag), fields(ident, filename = %filename))]
fn highlight(
    ident: u16,
    filename: Arc<str>,
    language: Option<languages::SharedConfig>,
    contents: bytes::Bytes,
    cancellation_flag: Arc<AtomicUsize>,
) -> HighlightOutput {
    let Some(language) = language else {
        return HighlightOutput::Failure {
            ident,
            filename,
            language: None,
            reason: NonFatalError::InvalidLanguage,
        };
    };

    let result: Result<_, tree_sitter_highlight::Error> = PER_THREAD.with_borrow_mut(|pt| {
        let iter = {
            let _span = tracing::trace_span!("highlight_with_tree_sitter").entered();
            pt.highlighter.highlight(
                &language.ts_config,
                &contents, // Zero-copy: Bytes derefs to &[u8]
                Some(&cancellation_flag),
                |_| None,
            )
        }?;

        let _span = tracing::trace_span!("render_html").entered();
        pt.renderer.reset();
        pt.renderer.render(iter, &contents, &callback)?;
        Ok(pt.renderer.lines().map(String::from).collect())
    });

    match result {
        Ok(lines) => HighlightOutput::Success {
            ident,
            filename,
            language,
            lines,
        },
        Err(err) => {
            tracing::Span::current().set_status(trace::Status::Error {
                description: err.to_string().into(),
            });
            HighlightOutput::Failure {
                ident,
                filename,
                language: Some(language),
                reason: NonFatalError::from(err),
            }
        }
    }
}

pub fn prepare_task(
    file: &html::File<'_>,
    body: Bytes,
    filename: Arc<str>,
    language_ptr: &mut Option<languages::SharedConfig>,
) -> Result<Bytes, NonFatalError> {
    // Look up the configured language from languages.rs
    *language_ptr = if file.language() == common::Language::Unspecified {
        // Infer language from filename
        languages::from_path(std::path::Path::new(filename.as_ref()))
    } else {
        // Convert FlatBuffers language to native Config
        file.language().try_into().ok()
    };

    let Some(language) = language_ptr else {
        Err(NonFatalError::InvalidLanguage)?
    };
    *language_ptr = Some(language);

    // Bail early before spawning a task, if there's no work to do.
    if file.contents().is_none_or(|s| s.is_empty()) {
        // We need a left_future here because Ready and Timeout<JoinHandle> are different future types,
        // even though they end up (after some .map() calls, in the latter case) returning the same type
        Err(NonFatalError::EmptyFile)?
    }

    // Check file size limit
    if file.contents().unwrap().bytes().len() > MAX_FILE_SIZE {
        Err(NonFatalError::FileTooLarge)?
    }

    // To avoid unnecessary copies, we slice out of the request body and
    // pass that memory location down to tree-sitter-highlight.
    let slice = file.contents().unwrap().bytes();
    let offset = slice.as_ptr() as usize - body.as_ptr() as usize;
    let contents = body.slice(offset..offset + slice.len());
    Ok(contents)
}

#[instrument(skip(state, body), fields(num_files, timeout_ms, request_size = body.len()))]
pub async fn html_handler(
    extract::State(state): extract::State<AppState>,
    body: Bytes,
) -> Result<axum::response::Response, FatalError> {
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
        return build_response(vec![]);
    }

    // This is the heart of the app: efficiently enqueuing concurrent highlighting requests,
    // propagating cancellation signals, and returning them in a stream, without
    // starving the tokio event loop and while processing as many documents as possible.
    let tasks: FuturesUnordered<_> = files
        .iter()
        .map(|file| {
            let ident = file.ident();
            let filename: Arc<str> = file.filename().unwrap_or_default().into();
            let mut language: Option<languages::SharedConfig> = None;
            // body.clone() here is not a full memory copy, because Bytes is cheap to clone
            let contents = match prepare_task(&file, body.clone(), filename.clone(), &mut language)
            {
                Ok(ok) => ok,
                Err(e) => {
                    return futures::future::ready(HighlightOutput::Failure {
                        ident,
                        filename,
                        language,
                        reason: e,
                    }).left_future()
                }
            };

            // Clone the cancellation flag and filename for error handlers
            let cancellation_flag = timeout_flag.clone();
            let cancellation_flag_for_timeout = cancellation_flag.clone();
            let filename_for_join_error = filename.clone();
            let filename_for_timeout = filename.clone();

            // Spawn a blocking task for highlighting this file
            let task = tokio::task::spawn_blocking(move || {
                highlight(
                    ident,
                    filename,
                    language,
                    contents,
                    cancellation_flag,
                )
            })
            .map(move |t| {
                // Fail gracefully if there was an error joining the thread
                // TODO: figure out how to signal this in a trace
                t.unwrap_or_else(|err| {
                    HighlightOutput::Failure {
                        ident,
                        filename: filename_for_join_error,
                        language,
                        reason: if err.is_cancelled() {
                            NonFatalError::Cancelled
                        } else {
                            NonFatalError::ThreadError
                        },
                    }
                })
            });
            // Run the task with the specified timeout
            tokio::time::timeout(timeout, task)
                .map(move |result| {
                    result.unwrap_or_else(|_elapsed| {
                        // Timeout occurred - set the cancellation flag so inflight tree-sitter-side tasks
                        // know that they should cancel and return.
                        cancellation_flag_for_timeout.store(1, Ordering::SeqCst);
                        HighlightOutput::Failure {
                            ident,
                            filename: filename_for_timeout,
                            language,
                            reason: NonFatalError::TimedOut,
                        }
                    })
                })
                .right_future()
        })
        .collect();
    // Wait on all in-flight tasks simultaneously with .collect() and build a response.
    build_response(tasks.collect().await)
}

async fn health_handler() -> &'static str {
    "ok"
}

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

pub fn router(default_per_file_timeout: Duration, max_per_file_timeout: Duration) -> Router {
    let state = AppState {
        default_per_file_timeout,
        max_per_file_timeout,
    };
    let counter = tower_http::metrics::in_flight_requests::InFlightRequestsCounter::new();

    use axum_tracing_opentelemetry::middleware;
    use tower_http::*;

    // Configure TraceLayer to include request ID in spans
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
        .layer(middleware::OtelInResponseLayer::default())
        .layer(middleware::OtelAxumLayer::default())
        .layer(extract::DefaultBodyLimit::max(MAX_REQUEST_SIZE));

    let app = Router::new()
        .route("/v1/html", post(html_handler))
        .route("/health", get(health_handler))
        .layer(layer)
        .with_state(state);
    app
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C), starting graceful shutdown");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, starting graceful shutdown");
        },
    }
}
