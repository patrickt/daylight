use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread_local;
use std::time::Duration;
use thiserror::Error;
use tree_sitter_highlight as ts;

use crate::daylight_generated::daylight::common::{ErrorCode, Language};
use crate::daylight_generated::daylight::html::{
    Document, DocumentArgs, Failure, FailureArgs,
    Request, Response as FbResponse, ResponseArgs,
};
use crate::languages as lang;

#[derive(Clone)]
struct AppState {
    default_per_file_timeout: Duration,
}

#[derive(Default)]
struct ThreadState {
    highlighter: ts::Highlighter,
    renderer: ts::HtmlRenderer,
}

thread_local! {
    pub static PER_THREAD: RefCell<ThreadState> = RefCell::default();
}

#[derive(Error, Debug)]
enum HighlightError {
    #[error("highlighting failed: {0}")]
    TreeSitter(#[from] tree_sitter_highlight::Error),
    #[error("unknown language")]
    UnknownLanguage,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl HighlightError {
    fn as_code(&self) -> ErrorCode {
        match self {
            Self::TreeSitter(tserr) => match tserr {
                ts::Error::Cancelled => ErrorCode::Cancelled,
                ts::Error::InvalidLanguage => ErrorCode::UnknownLanguage,
                ts::Error::Unknown => ErrorCode::Unspecified,
            },
            Self::UnknownLanguage => ErrorCode::UnknownLanguage,
            Self::InvalidRequest(_) | Self::Internal(_) => ErrorCode::Unspecified,
        }
    }
}

struct Job {
    ident: u16,
    filename: String,
    language: &'static lang::Config,
    // This would make more sense if it were a &[u8], but we can't send lifetimed
    // data across the thread pool boundary. So we slice it out of the axum request.
    contents: bytes::Bytes,
    timeout: Duration,
}

struct DocumentResult {
    ident: u16,
    filename: String,
    language: Language,
    lines: Vec<String>,
}

struct FailureResult {
    ident: u16,
    reason: HighlightError,
}

fn callback(highlight: ts::Highlight, output: &mut Vec<u8>) {
    let kind = lang::ALL_HIGHLIGHT_NAMES[highlight.0];
    output.extend(b"class=\"");
    output.extend(kind.as_bytes().iter());
    output.extend(b"\"")
}

fn parse(job: Job) -> Result<DocumentResult, FailureResult> {
    PER_THREAD
        .with_borrow_mut(|pt| {
            let cancellation_flag = Arc::new(AtomicUsize::new(0));

            // Spawn a task that will set the cancellation flag after timeout
            if !job.timeout.is_zero() {
                let flag_clone = cancellation_flag.clone();
                let timeout = job.timeout;
                std::thread::spawn(move || {
                    std::thread::sleep(timeout);
                    flag_clone.store(1, Ordering::Relaxed);
                });
            }

            let iter = pt.highlighter.highlight(
                &job.language.ts_config,
                &job.contents, // Zero-copy: Bytes derefs to &[u8]
                Some(&cancellation_flag),
                |_| None,
            )?;

            pt.renderer.reset();
            pt.renderer.render(iter, &job.contents, &callback)?;

            Ok(DocumentResult {
                ident: job.ident,
                filename: job.filename,
                language: job.language.fb_language,
                lines: pt.renderer.lines().map(String::from).collect(),
            })
        })
        .map_err(|err| FailureResult {
            ident: job.ident,
            reason: HighlightError::TreeSitter(err),
        })
}

// Handler for the /v1/html endpoint
async fn html_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, HighlightError> {
    let request = flatbuffers::root::<Request>(&body)
        .map_err(|e| HighlightError::InvalidRequest(e.to_string()))?;

    // TODO: we need some sort of cap on request time.
    let timeout_ms = request.timeout_ms();
    let timeout = if timeout_ms == 0 {
        state.default_per_file_timeout
    } else {
        Duration::from_millis(timeout_ms)
    };

    let files = request.files().ok_or_else(|| HighlightError::InvalidRequest("No files provided".to_string()))?;

    // Spawn a blocking task for each file
    let tasks: Vec<_> = files
        .iter()
        .map(|file| {
            let ident = file.ident();
            let filename = file.filename()
                .ok_or_else(|| HighlightError::InvalidRequest("Missing filename".to_string()))?
                .to_string();

            // Try to get language from request, otherwise infer from filename
            let language = match file.language() {
                Language::Unspecified => lang::from_path(Path::new(&filename))
                    .ok_or(HighlightError::UnknownLanguage)?,
                lang => lang.try_into()?,
            };

            // Get the contents bytes
            let contents_fb = file.contents()
                .ok_or_else(|| HighlightError::InvalidRequest("Missing file contents".to_string()))?;

            let contents_slice = contents_fb.bytes();

            // Calculate offset in the original buffer to create a zero-copy slice
            let offset = contents_slice.as_ptr() as usize - body.as_ptr() as usize;
            let contents = body.slice(offset..offset + contents_slice.len());

            // Spawn blocking task for this file
            Ok(tokio::task::spawn_blocking(move || {
                parse(Job {
                    ident,
                    filename,
                    language,
                    contents,
                    timeout,
                })
            }))
        })
        .collect::<Result<Vec<_>, HighlightError>>()?;

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    // Partition into failures and successes
    let mut failure_results = Vec::new();
    let mut doc_results = Vec::new();

    for result in results {
        match result {
            Ok(Ok(doc)) => doc_results.push(doc),
            Ok(Err(failure)) => failure_results.push(failure),
            Err(e) => return Err(HighlightError::Internal(format!("Task join error: {}", e))),
        }
    }

    // Build FlatBuffers response
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);

    // Build documents
    let documents: Vec<_> = doc_results
        .iter()
        .map(|doc| {
            let filename = builder.create_string(&doc.filename);
            let lines: Vec<_> = doc
                .lines
                .iter()
                .map(|line| builder.create_string(line))
                .collect();
            let lines_vec = builder.create_vector(&lines);

            Document::create(
                &mut builder,
                &DocumentArgs {
                    ident: doc.ident,
                    filename: Some(filename),
                    language: doc.language,
                    lines: Some(lines_vec),
                },
            )
        })
        .collect();

    let documents_vec = builder.create_vector(&documents);

    // Build failures
    let failures: Vec<_> = failure_results
        .iter()
        .map(|failure| {
            Failure::create(
                &mut builder,
                &FailureArgs {
                    ident: failure.ident,
                    reason: failure.reason.as_code(),
                },
            )
        })
        .collect();

    let failures_vec = builder.create_vector(&failures);

    // Build response
    let fb_response = FbResponse::create(
        &mut builder,
        &ResponseArgs {
            documents: Some(documents_vec),
            failures: Some(failures_vec),
        },
    );

    builder.finish(fb_response, None);
    let response_bytes = builder.finished_data();

    Ok((StatusCode::OK, response_bytes.to_vec()).into_response())
}

impl From<anyhow::Error> for HighlightError {
    fn from(err: anyhow::Error) -> Self {
        HighlightError::Internal(err.to_string())
    }
}

impl IntoResponse for HighlightError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            HighlightError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            HighlightError::TreeSitter(_) | HighlightError::UnknownLanguage | HighlightError::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
        };
        (status, message).into_response()
    }
}

pub async fn main(default_timeout: Duration, addr: SocketAddr) -> anyhow::Result<()> {
    let state = AppState {
        default_per_file_timeout: default_timeout,
    };

    let app = Router::new()
        .route("/v1/html", post(html_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
