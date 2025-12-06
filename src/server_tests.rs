use crate::daylight_generated::daylight::common;
use crate::daylight_generated::daylight::html;
use crate::server::*;
use axum::body::Bytes;
use axum::extract::State;
use http::StatusCode;
use quickcheck::TestResult;
use quickcheck_macros::quickcheck;
use std::sync::Arc;
use tokio::time::Duration;

// Helper to create a FlatBuffers request with given files
fn build_request(files: Vec<(u16, &str, &str, common::Language)>) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(4096);

    let fb_files: Vec<_> = files
        .iter()
        .map(|(ident, filename, contents, lang)| {
            let filename_offset = builder.create_string(filename);
            let contents_offset = builder.create_vector(contents.as_bytes());
            html::File::create(
                &mut builder,
                &html::FileArgs {
                    ident: *ident,
                    filename: Some(filename_offset),
                    contents: Some(contents_offset),
                    options: None,
                    language: *lang,
                },
            )
        })
        .collect();

    let files_vec = builder.create_vector(&fb_files);
    let request = html::Request::create(
        &mut builder,
        &html::RequestArgs {
            files: Some(files_vec),
            timeout_ms: 0,
        },
    );

    builder.finish(request, None);
    builder.finished_data().to_vec()
}

#[tokio::test]
async fn test_empty_request() {
    let state = AppState {
        default_per_file_timeout: Duration::from_secs(30),
        max_per_file_timeout: Duration::from_secs(60),
    };

    let request_bytes = build_request(vec![]);
    let response = html_handler(State(state), Bytes::from(request_bytes))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_single_c_file() {
    let state = AppState {
        default_per_file_timeout: Duration::from_secs(30),
        max_per_file_timeout: Duration::from_secs(60),
    };

    let c_code = r#"
#include <stdio.h>
int main() {
    printf("Hello, World!\n");
    return 0;
}
"#;

    let request_bytes = build_request(vec![(0, "test.c", c_code, common::Language::C)]);
    let response = html_handler(State(state), Bytes::from(request_bytes))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Parse response and verify structure
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let fb_response = flatbuffers::root::<html::Response>(&body).unwrap();

    assert!(fb_response.documents().is_some());
    let docs = fb_response.documents().unwrap();
    assert_eq!(docs.len(), 1);

    let doc = docs.get(0);
    assert_eq!(doc.ident(), 0);
    assert_eq!(doc.error_code(), common::ErrorCode::NoError);
    assert!(doc.lines().is_some());
    assert!(doc.lines().unwrap().len() > 0);
}

#[tokio::test]
async fn test_empty_file_contents() {
    let state = AppState {
        default_per_file_timeout: Duration::from_secs(30),
        max_per_file_timeout: Duration::from_secs(60),
    };

    let request_bytes = build_request(vec![(0, "empty.c", "", common::Language::C)]);
    let response = html_handler(State(state), Bytes::from(request_bytes))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let fb_response = flatbuffers::root::<html::Response>(&body).unwrap();

    let docs = fb_response.documents().unwrap();
    assert_eq!(docs.len(), 1);
    let doc = docs.get(0);
    assert_eq!(doc.error_code(), common::ErrorCode::NoError);
}

#[tokio::test]
async fn test_multiple_files_concurrently() {
    let state = AppState {
        default_per_file_timeout: Duration::from_secs(30),
        max_per_file_timeout: Duration::from_secs(60),
    };

    let files = vec![
        (0, "test1.c", "int main() { return 0; }", common::Language::C),
        (1, "test2.c", "void foo() {}", common::Language::C),
        (
            2,
            "test3.bash",
            "#!/bin/bash\necho hello",
            common::Language::Bash,
        ),
    ];

    let request_bytes = build_request(files);
    let response = html_handler(State(state), Bytes::from(request_bytes))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let fb_response = flatbuffers::root::<html::Response>(&body).unwrap();

    let docs = fb_response.documents().unwrap();
    assert_eq!(docs.len(), 3);

    for doc in docs.iter() {
        assert_eq!(doc.error_code(), common::ErrorCode::NoError);
        assert!(doc.lines().is_some());
    }
}

#[tokio::test]
async fn test_timeout_too_large() {
    let state = AppState {
        default_per_file_timeout: Duration::from_secs(30),
        max_per_file_timeout: Duration::from_secs(60),
    };

    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);
    let files_vec = builder.create_vector::<flatbuffers::WIPOffset<html::File>>(&[]);
    let request = html::Request::create(
        &mut builder,
        &html::RequestArgs {
            files: Some(files_vec),
            timeout_ms: 120_000, // 120 seconds, exceeds max of 60
        },
    );
    builder.finish(request, None);
    let request_bytes = builder.finished_data().to_vec();

    let response = html_handler(State(state), Bytes::from(request_bytes)).await;

    assert!(response.is_err());
}

// Property: even garbage sent down the line should still be reified in the result
#[quickcheck]
fn prop_arbitrary_input_still_produces_response(code: String) -> TestResult {
    // Skip empty or overly long strings
    if code.is_empty() || code.len() > 10000 {
        return TestResult::discard();
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let state = AppState {
            default_per_file_timeout: Duration::from_secs(5),
            max_per_file_timeout: Duration::from_secs(10),
        };

        let request_bytes = build_request(vec![(0, "test.c", &code, common::Language::C)]);
        let response = html_handler(State(state), Bytes::from(request_bytes))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let fb_response = flatbuffers::root::<html::Response>(&body).unwrap();

        let docs = fb_response.documents().unwrap();
        assert_eq!(docs.len(), 1);

        TestResult::passed()
    })
}

// Property: Document idents should match request idents
#[quickcheck]
fn prop_idents_preserved(idents: Vec<u16>) -> TestResult {
    if idents.is_empty() || idents.len() > 100 {
        return TestResult::discard();
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let state = AppState {
            default_per_file_timeout: Duration::from_secs(5),
            max_per_file_timeout: Duration::from_secs(10),
        };

        let files: Vec<_> = idents
            .iter()
            .map(|&id| (id, "test.c", "int main() {}", common::Language::C))
            .collect();

        let request_bytes = build_request(files);
        let response = html_handler(State(state), Bytes::from(request_bytes))
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let fb_response = flatbuffers::root::<html::Response>(&body).unwrap();

        let docs = fb_response.documents().unwrap();
        assert_eq!(docs.len(), idents.len());

        let mut returned_idents: Vec<u16> = docs.iter().map(|d| d.ident()).collect();
        let mut expected_idents = idents.clone();
        returned_idents.sort();
        expected_idents.sort();

        TestResult::from_bool(returned_idents == expected_idents)
    })
}
