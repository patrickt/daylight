use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::body::Bytes;
use axum::response::IntoResponse;
use http::StatusCode;
use tracing::instrument;
use tree_sitter_highlight as ts;

use crate::daylight_generated::daylight::spans;
use crate::errors::FatalError;
use crate::languages::{self, ALL_HIGHLIGHT_NAMES};
use crate::thread_locals::ThreadState;

use super::{Outcome, Processor};

/// Spans processor that returns numeric highlight span information.
pub struct SpansProcessor;

impl Processor for SpansProcessor {
    type Output = (usize, usize, usize);

    fn process(
        ident: u16,
        filename: Arc<str>,
        language: languages::SharedConfig,
        contents: Bytes,
        include_injections: bool,
        cancellation_flag: Arc<AtomicUsize>,
    ) -> Outcome<(usize, usize, usize)> {
        ThreadState::highlight_with_tree_sitter(|highlighter| {
            let iter_res = {
                highlighter.highlight(
                    &language.ts_config,
                    &contents,
                    Some(&cancellation_flag),
                    |s| {
                        if include_injections {
                            languages::from_name(s).map(|l| &l.ts_config)
                        } else {
                            None
                        }
                    },
                )
            };

            let iter = match iter_res {
                Ok(iter) => iter,
                Err(e) => {
                    return Outcome::failure(ident, filename, None, e.into());
                }
            };

            let mut spans: Vec<(usize, usize, usize)> = vec![];
            let mut active_index: Option<usize> = None;
            use ts::HighlightEvent;
            for item in iter {
                if let Ok(evt) = item {
                    match (evt, active_index) {
                        (HighlightEvent::Source { start, end }, Some(active)) => {
                            spans.push((active, start, end))
                        }
                        (HighlightEvent::HighlightStart(highlight), None) => {
                            active_index = Some(highlight.0)
                        }
                        (HighlightEvent::HighlightEnd, None) => active_index = None,
                        _ => tracing::warn!("Unexpected event {evt:?} with index {active_index:?}"),
                    }
                }
            }
            Outcome::Success {
                ident,
                filename,
                language,
                contents: spans,
            }
        })
    }

    #[instrument(skip(outputs), fields(count = outputs.len()))]
    fn build_response(
        outputs: Vec<Outcome<(usize, usize, usize)>>,
    ) -> Result<axum::response::Response, FatalError> {
        ThreadState::build_flatbuffers(|mut builder| {
            builder.reset();
            let documents = outputs
                .into_iter()
                .map(|doc| {
                    let filename = builder.create_string(doc.filename());
                    let spans = match doc {
                        Outcome::Success { ref contents, .. } => {
                            let line_offsets: Vec<_> = contents
                                .into_iter()
                                .map(|line| {
                                    spans::Span::create(
                                        &mut builder,
                                        &spans::SpanArgs {
                                            index: line.0 as u16,
                                            start: line.1 as u64,
                                            end: line.2 as u64,
                                        },
                                    )
                                })
                                .collect();
                            Some(builder.create_vector(&line_offsets))
                        }
                        _ => None,
                    };
                    spans::Document::create(
                        &mut builder,
                        &spans::DocumentArgs {
                            ident: doc.ident(),
                            filename: Some(filename),
                            language: doc.language(),
                            spans,
                            error_code: doc.error_code(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            let documents = Some(builder.create_vector(&documents));
            let highlight_names = ALL_HIGHLIGHT_NAMES
                .into_iter()
                .map(String::from)
                .map(|s| builder.create_string(&s))
                .collect::<Vec<_>>();
            let highlight_names = Some(builder.create_vector(&highlight_names));

            let response = spans::Response::create(
                &mut builder,
                &spans::ResponseArgs {
                    documents,
                    highlight_names,
                },
            );
            builder.finish(response, None);
            let response_bytes = builder.finished_data();
            Ok((StatusCode::OK, Bytes::copy_from_slice(response_bytes)).into_response())
        })
    }
}
