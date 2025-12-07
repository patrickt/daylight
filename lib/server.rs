use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::daylight_generated::daylight::common::{self};
use crate::daylight_generated::daylight::html;
use crate::languages;
use axum::{body::Bytes, extract::State, response::IntoResponse, routing::post, Router};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use http::StatusCode;
use thiserror::Error;
use tokio::time::Duration;
use tracing::instrument;
use tree_sitter_highlight as ts;

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
pub enum HtmlError {
    #[error("Decoding request failed")]
    DecodeError(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("Timeout too large (max supported: {max}ms)", max = .0.as_millis())]
    TimeoutTooLarge(Duration),
}

impl IntoResponse for HtmlError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}

fn build_response(
    doc_results: Vec<OwnedDocument>
) -> Result<axum::response::Response, HtmlError> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);

    // Build documents
    let documents: Vec<_> = doc_results
        .into_iter()
        .map(|doc| {
            let filename = builder.create_string(&doc.filename);

            let lines: Vec<_> = doc
                .lines
                .iter()
                .map(|line| builder.create_string(line))
                .collect();
            let lines_vec = builder.create_vector(&lines);

            html::Document::create(
                &mut builder,
                &html::DocumentArgs {
                    ident: doc.ident,
                    filename: Some(filename),
                    language: doc.language,
                    lines: Some(lines_vec),
                    error_code: doc.error_code,
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

pub struct OwnedDocument {
    pub ident: u16,
    pub filename: Arc<str>,
    pub language: common::Language,
    pub lines: Vec<String>,
    pub error_code: common::ErrorCode,
}

impl OwnedDocument {
    pub fn error(
        ident: u16,
        filename: Arc<str>,
        language: common::Language,
        error_code: common::ErrorCode,
    ) -> Self {
        Self {
            ident,
            filename,
            language,
            lines: Vec::new(),
            error_code,
        }
    }
}

fn callback(highlight: ts::Highlight, output: &mut Vec<u8>) {
    let kind = languages::ALL_HIGHLIGHT_NAMES[highlight.0];
    output.extend_from_slice(b"class=\"");
    output.extend_from_slice(kind.as_bytes());
    output.extend_from_slice(b"\"");
}

#[instrument(skip(language, contents, cancellation_flag), fields(ident, filename = %filename, language = language.name))]
fn highlight(
    ident: u16,
    filename: Arc<str>,
    language: &'static languages::Config,
    contents: bytes::Bytes,
    cancellation_flag: Arc<AtomicUsize>,
) -> OwnedDocument {
    let result = PER_THREAD.with_borrow_mut(|pt| {
        let iter = pt.highlighter.highlight(
            &language.ts_config,
            &contents, // Zero-copy: Bytes derefs to &[u8]
            Some(&cancellation_flag),
            |_| None,
        )?;

        pt.renderer.reset();
        pt.renderer.render(iter, &contents, &callback)?;

        Ok(pt.renderer.lines().map(String::from).collect())
    });

    match result {
        Ok(lines) => OwnedDocument {
            ident,
            filename,
            language: language.fb_language,
            lines,
            error_code: common::ErrorCode::NoError,
        },
        Err(err) => {
            let error_code = match err {
                ts::Error::Cancelled => common::ErrorCode::Cancelled,
                ts::Error::InvalidLanguage => common::ErrorCode::UnknownLanguage,
                ts::Error::Unknown => common::ErrorCode::UnknownError,
            };
            OwnedDocument::error(ident, filename, language.fb_language, error_code)
        }
    }
}

#[instrument(skip(state, body), fields(num_files, timeout_ms, request_size = body.len()))]
pub async fn html_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<axum::response::Response, HtmlError> {
    let request = flatbuffers::root::<html::Request>(&body)?;

    let timeout_ms = request.timeout_ms();
    let timeout = if timeout_ms == 0 {
        state.default_per_file_timeout
    } else {
        Duration::from_millis(timeout_ms)
    };
    if timeout > state.max_per_file_timeout {
        Err(HtmlError::TimeoutTooLarge(state.max_per_file_timeout))?
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
            // Pull out invariant values
            let ident = file.ident();
            let filename: Arc<str> = Arc::from(file.filename().unwrap_or_default());
            let language = file.language();

            // Bail early before spawning a task, if there's no work to do.
            if file.contents().is_none_or(|s| s.is_empty()) {
                // We need a left_future here because Ready and Timeout<JoinHandle> are different future types,
                // even though they end up (after some .map() calls, in the latter case) returning the same type
                return futures::future::ready(OwnedDocument::error(
                    ident,
                    filename,
                    language,
                    common::ErrorCode::NoError,
                ))
                .left_future();
            }

            // Look up the configured language from languages.rs
            let native_language = if file.language() == common::Language::Unspecified {
                // Infer language from filename
                languages::from_path(std::path::Path::new(filename.as_ref()))
            } else {
                // Convert FlatBuffers language to native Config
                file.language().try_into().ok()
            };

            let Some(native_language) = native_language else {
                return futures::future::ready(OwnedDocument::error(
                    ident,
                    filename.clone(),
                    language,
                    common::ErrorCode::UnknownLanguage,
                ))
                .left_future();
            };
            // To avoid unnecessary copies, we slice out of the request body and
            // pass that memory location down to tree-sitter-highlight.
            let slice = file.contents().unwrap().bytes();
            let offset = slice.as_ptr() as usize - body.as_ptr() as usize;
            let contents = body.slice(offset..offset + slice.len());

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
                    native_language,
                    contents,
                    cancellation_flag,
                )
            })
            .map(move |t| {
                // Fail gracefully if there was an error joining the thread
                t.unwrap_or_else(|err| {
                    OwnedDocument::error(
                        ident,
                        filename_for_join_error,
                        language,
                        if err.is_cancelled() {
                            common::ErrorCode::Cancelled
                        } else {
                            common::ErrorCode::UnknownError
                        },
                    )
                })
            });
            // Run the task with the specified timeout
            tokio::time::timeout(timeout, task).map(move |result| {
                result.unwrap_or_else(|_elapsed| {
                    // Timeout occurred - set the cancellation flag so inflight tree-sitter-side tasks
                    // know that they should cancel and return.
                    cancellation_flag_for_timeout.store(1, Ordering::SeqCst);
                    OwnedDocument::error(
                        ident,
                        filename_for_timeout,
                        language,
                        common::ErrorCode::TimedOut,
                    )
                })
            }).right_future()
        })
        .collect();
    // Wait on all in-flight tasks simultaneously with .collect() and build a response.
    build_response(tasks.collect().await)
}

pub async fn run(
    default_per_file_timeout: Duration,
    max_per_file_timeout: Duration,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let state = AppState {
        default_per_file_timeout,
        max_per_file_timeout,
    };

    let app = Router::new()
        .route("/v1/html", post(html_handler))
        .layer(axum_tracing_opentelemetry::middleware::OtelAxumLayer::default())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
