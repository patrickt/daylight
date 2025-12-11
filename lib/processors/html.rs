use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::body::Bytes;
use axum::response::IntoResponse;
use http::StatusCode;
use opentelemetry::trace;
use tracing::{Span, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tree_sitter_highlight as ts;

use crate::daylight_generated::daylight::html;
use crate::errors::{FatalError, NonFatalError};
use crate::languages;
use crate::thread_locals::ThreadState;

use super::{Outcome, Processor};

/// HTML processor that returns formatted HTML strings.
pub struct HtmlProcessor;

impl Processor for HtmlProcessor {
    type Output = String;

    #[instrument(skip(language, contents, cancellation_flag))]
    fn process(
        ident: u16,
        filename: Arc<str>,
        language: languages::SharedConfig,
        contents: Bytes,
        include_injections: bool,
        cancellation_flag: Arc<AtomicUsize>,
    ) -> Outcome<String> {
        let result = ThreadState::highlight_with_tree_sitter(|highlighter| {
            let iter = {
                let _span = tracing::trace_span!("highlight_with_tree_sitter").entered();
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
            }?;

            ThreadState::render_with_tree_sitter(|renderer| {
                renderer.reset();
                renderer.render(iter, &contents, &|highlight, output| {
                    let kind = languages::ALL_HIGHLIGHT_NAMES[highlight.0];
                    output.extend_from_slice(b"class=\"");
                    output.extend_from_slice(kind.as_bytes());
                    output.extend_from_slice(b"\"");
                })?;
                Ok(renderer.lines().map(String::from).collect())
            })
        })
        .map_err(|e: ts::Error| NonFatalError::from(e));

        match result {
            Ok(lines) => Outcome::Success {
                ident,
                filename,
                language,
                contents: lines,
            },
            Err(err) => {
                Span::current().set_status(trace::Status::Error {
                    description: err.to_string().into(),
                });
                Outcome::Failure {
                    ident,
                    filename,
                    language: Some(language),
                    reason: NonFatalError::from(err),
                }
            }
        }
    }

    #[instrument(skip(outputs), fields(count = outputs.len()))]
    fn build_response(
        outputs: Vec<Outcome<String>>,
    ) -> Result<axum::response::Response, FatalError> {
        ThreadState::build_flatbuffers(|mut builder| {
            builder.reset();
            let documents = outputs
                .into_iter()
                .map(|doc| {
                    let filename = builder.create_string(doc.filename());
                    let lines = match doc {
                        Outcome::Success { ref contents, .. } => {
                            let line_offsets: Vec<_> = contents
                                .into_iter()
                                .map(|line| builder.create_string(line))
                                .collect();
                            Some(builder.create_vector(&line_offsets))
                        }
                        _ => None,
                    };
                    html::Document::create(
                        &mut builder,
                        &html::DocumentArgs {
                            ident: doc.ident(),
                            filename: Some(filename),
                            language: doc.language(),
                            lines,
                            error_code: doc.error_code(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            let documents = Some(builder.create_vector(&documents));
            let response = html::Response::create(&mut builder, &html::ResponseArgs { documents });
            builder.finish(response, None);
            let response_bytes = builder.finished_data();
            Ok((StatusCode::OK, Bytes::copy_from_slice(response_bytes)).into_response())
        })
    }
}
