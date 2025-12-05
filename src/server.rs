use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

use futures::AsyncReadExt;
use rayon::prelude::*;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread_local;
use std::time::Duration;
use thiserror::Error;
use tree_sitter_highlight as ts;

use crate::daylight_capnp::html_highlighter as html;
use crate::{daylight_capnp, languages as lang};

struct Daylight {
    pool: rayon::ThreadPool,
    per_file_timeout: Duration,
}

#[derive(Default)]
struct PerThread {
    highlighter: ts::Highlighter,
    renderer: ts::HtmlRenderer,
}

thread_local! {
    pub static RESOURCES: RefCell<PerThread> = RefCell::default();
}

impl Daylight {
    fn new(num_threads: usize, per_file_timeout: Duration) -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(num_threads).build()?;
        Ok(Daylight { pool, per_file_timeout })
    }
}

#[derive(Error, Debug)]
enum HighlightError {
    #[error("highlighting failed: {0}")]
    TreeSitter(#[from] tree_sitter_highlight::Error),
    #[error("unknown language")]
    UnknownLanguage,
}

impl HighlightError {
    fn as_code(&self) -> daylight_capnp::ErrorCode {
        match self {
            Self::TreeSitter(tserr) => match tserr {
                ts::Error::Cancelled => daylight_capnp::ErrorCode::Cancelled,
                ts::Error::InvalidLanguage => daylight_capnp::ErrorCode::UnknownLanguage,
                ts::Error::Unknown => daylight_capnp::ErrorCode::Unspecified,
            },
            Self::UnknownLanguage => daylight_capnp::ErrorCode::UnknownLanguage,
        }
    }
}

impl From<HighlightError> for capnp::Error {
    fn from(err: HighlightError) -> Self {
        capnp::Error::failed(err.to_string())
    }
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
    language: crate::daylight_capnp::Language,
    lines: Vec<String>,
}

struct FailureResult {
    ident: u16,
    reason: HighlightError,
}

fn callback(_highlight: ts::Highlight, _span: &mut Vec<u8>) {}

fn parse(job: FileJob) -> Result<DocumentResult, FailureResult> {
    RESOURCES
        .with_borrow_mut(|pt| {
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

impl html::Server for Daylight {
    async fn html(
        self: Rc<Self>,
        params: html::HtmlParams,
        mut results: html::HtmlResults,
    ) -> Result<(), capnp::Error> {
        let request = params.get()?.get_request()?;
        let files = request.get_files()?;
        let timeout_ms = request.get_timeout_ms();
        let timeout = if timeout_ms == 0 {
            self.per_file_timeout
        } else {
            Duration::from_millis(timeout_ms)
        };

        let jobs: Result<Vec<FileJob>, capnp::Error> = files
            .iter()
            .map(|file| {
                let filename = file.get_filename()?.to_string()?;

                // Try to get language from request, otherwise infer from filename
                let language = match file.get_language()? {
                    daylight_capnp::Language::Unspecified => lang::from_path(Path::new(&filename))
                        .ok_or(HighlightError::UnknownLanguage)?,

                    lang => lang.try_into()?,
                };

                Ok(FileJob {
                    ident: file.get_ident(),
                    filename,
                    language,
                    contents: file.get_contents()?.to_vec(),
                    timeout,
                })
            })
            .collect();
        let jobs = jobs?;

        let (failure_results, doc_results): (Vec<FailureResult>, Vec<DocumentResult>) = self
            .pool
            .install(|| jobs.into_par_iter().partition_map(|x| parse(x).into()));

        let mut response = results.get().init_response();
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
                failure.set_reason(failure_result.reason.as_code())
            }
        }

        Ok(())
    }
}

pub async fn main(num_threads: usize, default_timeout: Duration, addr: SocketAddr) -> anyhow::Result<()> {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            let daylight = Daylight::new(num_threads, default_timeout)?;
            let daylight_client: html::Client = capnp_rpc::new_client(daylight);

            loop {
                let (stream, _) = listener.accept().await?;
                stream.set_nodelay(true)?;
                let (reader, writer) =
                    tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
                let network = twoparty::VatNetwork::new(
                    futures::io::BufReader::new(reader),
                    futures::io::BufWriter::new(writer),
                    rpc_twoparty_capnp::Side::Server,
                    Default::default(),
                );

                let rpc_system =
                    RpcSystem::new(Box::new(network), Some(daylight_client.clone().client));

                tokio::task::spawn_local(rpc_system);
            }
        })
        .await
}
