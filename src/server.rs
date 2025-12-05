use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Router,
};
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread_local;
use std::cell::RefCell;
use std::time::Duration;
use thiserror::Error;
use tree_sitter_highlight as ts;

use crate::{daylight_capnp, languages as lang};

#[derive(Clone)]
pub struct AppState {
    pool: Arc<rayon::ThreadPool>,
}

#[derive(Default)]
struct PerThread {
    highlighter: ts::Highlighter,
    renderer: ts::HtmlRenderer,
}

thread_local! {
    pub static RESOURCES: RefCell<PerThread> = RefCell::default();
}

#[derive(Error, Debug)]
enum HighlightError {
    #[error("highlighting failed: {0}")]
    TreeSitter(#[from] tree_sitter_highlight::Error),
}

struct FileJob {
    ident: u16,
    filename: String,
    language: &'static lang::Language,
    contents: Vec<u8>,
    timeout: Duration,
}

struct DocumentResult {
    ident: u16,
    filename: String,
    language: daylight_capnp::Language,
    lines: Vec<String>,
}

struct FailureResult {
    ident: u16,
    reason: HighlightError,
}

impl AppState {
    pub fn new() -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
        Ok(AppState {
            pool: Arc::new(pool),
        })
    }
}

impl HighlightError {
    fn as_code(&self) -> daylight_capnp::ErrorCode {
        match self {
            Self::TreeSitter(tserr) => match tserr {
                ts::Error::Cancelled => daylight_capnp::ErrorCode::Cancelled,
                ts::Error::InvalidLanguage => daylight_capnp::ErrorCode::UnknownLanguage,
                ts::Error::Unknown => daylight_capnp::ErrorCode::Unspecified,
            }
        }
    }
}

fn callback(_highlight: ts::Highlight, _span: &mut Vec<u8>) {}

fn parse(job: FileJob) -> Result<DocumentResult, FailureResult> {
    RESOURCES.with_borrow_mut(|pt| {
        let cancellation_flag = Arc::new(AtomicUsize::new(0));

        // Spawn a task that will set the cancellation flag after timeout
        if !job.timeout.is_zero() {
            let flag_clone = cancellation_flag.clone();
            let timeout = job.timeout;
            rayon::spawn(move || {
                std::thread::sleep(timeout);
                flag_clone.store(1, Ordering::Relaxed);
            });
        }

        let iter = pt.highlighter.highlight(
            &job.language.ts_config,
            &job.contents,
            Some(&cancellation_flag),
            |_| None,
        )?;

        pt.renderer.reset();
        pt.renderer.render(iter, &job.contents, &callback)?;

        Ok(DocumentResult {
            ident: job.ident,
            filename: job.filename,
            language: job.language.capnp_language,
            lines: pt.renderer.lines().map(String::from).collect(),
        })
    })
    .map_err(|err| FailureResult {
        ident: job.ident,
        reason: HighlightError::TreeSitter(err),
    })
}

async fn html_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Vec<u8>, StatusCode> {
    // Parse Cap'n Proto request
    let message_reader = capnp::serialize::read_message(
        &mut &body[..],
        capnp::message::ReaderOptions::new(),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;

    let request = message_reader
        .get_root::<daylight_capnp::request::Reader>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let files = request.get_files().map_err(|_| StatusCode::BAD_REQUEST)?;
    let timeout = Duration::from_millis(request.get_timeout_ms());

    // Build jobs
    let jobs: Result<Vec<FileJob>, capnp::Error> = files
        .iter()
        .map(|file| {
            Ok(FileJob {
                ident: file.get_ident(),
                filename: file.get_filename()?.to_string()?,
                language: file.get_language()?.try_into()?,
                contents: file.get_contents()?.to_vec(),
                timeout,
            })
        })
        .collect();

    let jobs = jobs.map_err(|_| StatusCode::BAD_REQUEST)?;

    // Process in parallel
    let (failure_results, doc_results): (Vec<FailureResult>, Vec<DocumentResult>) = state
        .pool
        .install(|| jobs.into_par_iter().partition_map(|x| parse(x).into()));

    // Build Cap'n Proto response
    let mut message = capnp::message::Builder::new_default();
    let mut response = message.init_root::<daylight_capnp::response::Builder>();

    {
        let mut documents = response.reborrow().init_documents(doc_results.len() as u32);
        for (i, doc_result) in doc_results.iter().enumerate() {
            let mut doc = documents.reborrow().get(i as u32);
            doc.set_ident(doc_result.ident);
            doc.set_filename(&doc_result.filename);
            doc.set_language(doc_result.language);
            let mut lines = doc.init_lines(doc_result.lines.len() as u32);
            for (j, line) in doc_result.lines.iter().enumerate() {
                lines.set(j as u32, line);
            }
        }
    }

    {
        let mut failures = response
            .reborrow()
            .init_failures(failure_results.len() as u32);
        for (i, failure_result) in failure_results.iter().enumerate() {
            let mut failure = failures.reborrow().get(i as u32);
            failure.set_ident(failure_result.ident);
            failure.set_reason(failure_result.reason.as_code());
        }
    }

    // Serialize response
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(buf)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/html", post(html_handler))
        .with_state(state)
}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        println!("usage: {} server ADDRESS[:PORT]", args[0]);
        return Ok(());
    }

    let state = AppState::new()?;
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&args[2]).await?;
    println!("Listening on {}", listener.local_addr()?);

    axum::serve(listener, app).await?;

    Ok(())
}
