use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

use futures::AsyncReadExt;
use rayon::prelude::*;
use std::cell::RefCell;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread_local;
use std::time::Duration;
use thiserror::Error;
use tree_sitter_highlight as ts;

use crate::{daylight_capnp, languages as lang};
use crate::daylight_capnp::html_highlighter as html;

struct Daylight {
    pool: rayon::ThreadPool,
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
    fn new() -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
        Ok(Daylight{pool})
    }
}

#[derive(Error, Debug)]
enum HighlightError {
    #[error("highlighting failed: {0}")]
    TreeSitter(#[from] tree_sitter_highlight::Error)
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

fn callback(_highlight: ts::Highlight, _span: &mut Vec<u8>) {
}

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
    }).map_err(|err| {
        FailureResult {
            ident: job.ident,
            reason: HighlightError::TreeSitter(err),
        }
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
        let timeout = Duration::from_millis(request.get_timeout_ms());

        let jobs: Result<Vec<FileJob>, capnp::Error> = files.iter()
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
        let jobs = jobs?;

        let (failure_results, doc_results): (Vec<FailureResult>, Vec<DocumentResult>) =
            self.pool.install(|| jobs.into_par_iter().partition_map(|x| parse(x).into()));

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
            let mut failures = response.reborrow().init_failures(failure_results.len() as u32);
            for (i, failure_result) in failure_results.iter().enumerate() {
                let mut failure = failures.reborrow().get(i as u32);
                failure.set_ident(failure_result.ident);
                failure.set_reason(failure_result.reason.as_code())
            }
        }

        Ok(())
    }

}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        println!("usage: {} server ADDRESS[:PORT]", args[0]);
        return Ok(());
    }

    let addr = args[2]
        .to_socket_addrs()?
        .next()
        .expect("could not parse address");

    tokio::task::LocalSet::new()
        .run_until(async move {
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let daylight = Daylight::new()?;
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
