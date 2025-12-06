use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{Router, routing::post, body::Bytes, extract::State, response::IntoResponse};
use futures::{FutureExt, StreamExt};
use futures::{future::Ready, stream::FuturesUnordered};
use http::StatusCode;
use thiserror::Error;
use tokio::time::Duration;
use tree_sitter_highlight as ts;
use crate::daylight_generated::daylight::common::{self};
use crate::daylight_generated::daylight::html;
use crate::languages;

#[derive(Clone)]
struct AppState {
    default_per_file_timeout: Duration,
    max_per_file_timeout: Duration,
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
enum HtmlError {
    #[error("Decoding request failed")]
    DecodeError(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("Timeout too large (max supported: {max}ms)", max = .0.as_millis())]
    TimeoutTooLarge(Duration),
    #[error("Internal service error: {0}")]
    #[allow(dead_code)]
    Internal(String),
}

impl IntoResponse for HtmlError {
    fn into_response(self) -> axum::response::Response {
        use HtmlError::*;
        let code = match self {
            Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        (code, self.to_string()).into_response()
    }
}

fn build_response(doc_results: Vec<OwnedDocument>) -> Result<axum::response::Response, HtmlError> {
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

    Ok((StatusCode::OK, response_bytes.to_vec()).into_response())
}

struct OwnedDocument {
    ident: u16,
    filename: String,
    language: common::Language,
    lines: Vec<String>,
    error_code: common::ErrorCode,
}

fn callback(highlight: ts::Highlight, output: &mut Vec<u8>) {
    let kind = languages::ALL_HIGHLIGHT_NAMES[highlight.0];
    output.extend(b"class=\"");
    output.extend(kind.as_bytes().iter());
    output.extend(b"\"")
}

fn parse(
    ident: u16,
    filename: String,
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

        Ok::<_, tree_sitter_highlight::Error>(pt.renderer.lines().map(String::from).collect())
    });

    match result {
        Ok(lines) => OwnedDocument {
            ident,
            filename,
            language: language.fb_language,
            lines,
            error_code: common::ErrorCode::NoError,
        },
        Err(err) => OwnedDocument {
            ident,
            filename,
            language: language.fb_language,
            lines: Vec::new(),
            error_code: match err {
                ts::Error::Cancelled => common::ErrorCode::Cancelled,
                ts::Error::InvalidLanguage => common::ErrorCode::UnknownLanguage,
                ts::Error::Unknown => common::ErrorCode::UnknownError,
            },
        },
    }
}

async fn html_handler(State(state): State<AppState>, body: Bytes) -> Result<axum::response::Response, HtmlError> {
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
    if files.is_empty() {
        return build_response(vec![]);
    }

    let tasks: FuturesUnordered<futures::future::Either<Ready<OwnedDocument>, _>> =
        files.iter().map(|file| {
            let mut failure = OwnedDocument {
                ident: file.ident(),
                lines: vec![],
                filename: file.filename().unwrap_or_default().to_string(),
                language: file.language(),
                error_code: common::ErrorCode::NoError,
            };
            if file.contents().is_none_or(|s| s.is_empty()) {
                return futures::future::ready(failure).left_future()
            }
            let ident = file.ident();
            let filename = String::from(file.filename().unwrap_or_default());
            let language = match file.language() {
                common::Language::Unspecified => todo!(),
                lang => lang.try_into().unwrap(),
            };
           // Get the contents bytes - zero-copy slice from request buffer
            let slice = file.contents()
                .unwrap()
                .bytes();
            let offset = slice.as_ptr() as usize - body.as_ptr() as usize;
            let contents = body.slice(offset..offset + slice.len());
            let cancellation_flag = timeout_flag.clone();
            let cancellation_flag_for_timeout = cancellation_flag.clone();
            let task = tokio::task::spawn_blocking(move || {
                parse(ident, filename, language, contents, cancellation_flag)
            }).map(move |t| {
                t.unwrap_or(OwnedDocument {
                    ident: file.ident(),
                    lines: vec![],
                    filename: file.filename().unwrap_or_default().to_string(),
                    language: file.language(),
                    error_code: common::ErrorCode::UnknownError,
                })
            });
            let to = tokio::time::timeout(timeout, task);
            let timeout_handled = to.map(move |result| {
                result.unwrap_or_else(|_elapsed| {
                    // Timeout occurred - set cancellation flag and return timed out document
                    cancellation_flag_for_timeout.store(1, Ordering::Relaxed);
                    failure.error_code = common::ErrorCode::TimedOut;
                    failure
                })
            });
            timeout_handled.right_future()
        }).collect();

    let results: Vec<OwnedDocument> = tasks.collect().await;
    build_response(results)
}

pub async fn main(
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
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
