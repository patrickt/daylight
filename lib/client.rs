use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use flatbuffers::{FlatBufferBuilder, InvalidFlatbuffer};
use thiserror::Error;

pub use crate::daylight_generated::daylight::common;
pub use crate::daylight_generated::daylight::html;
pub use crate::languages::SharedConfig;

pub struct Client<'a> {
    url: String,
    http: reqwest::Client,
    builder: FlatBufferBuilder<'a>,
    files: Vec<flatbuffers::WIPOffset<common::File<'a>>>,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("Couldn't decode Flatbuffers response: {0}")]
    DecodingError(#[from] InvalidFlatbuffer),
    #[error("Timeout too large ({0}ms)")]
    TimeoutTooLarge(u128),
}

impl<'a> Client<'a> {
    pub fn new() -> Self {
        Self {
            url: "localhost".to_string(),
            http: reqwest::Client::new(),
            builder: Default::default(),
            files: vec![],
        }
    }

    pub fn add_file(
        &mut self,
        ident: u16,
        filename: Option<&str>,
        contents: &[u8],
        language: SharedConfig,
        include_injections: bool,
    ) {
        let file = common::FileArgs {
            ident,
            filename: filename.map(|f| self.builder.create_string(f)),
            contents: Some(self.builder.create_vector(contents)),
            options: None,
            language: language.fb_language,
            include_injections,
        };
        self.files.push(common::File::create(&mut self.builder, &file))
    }

    pub async fn html(&mut self, timeout: Duration) -> Result<Bytes, Error> {
        let all_files = self.builder.create_vector(&self.files);
        let request = html::Request::create(
            &mut self.builder,
            &html::RequestArgs {
                files: Some(all_files),
                timeout_ms: timeout
                    .as_millis()
                    .try_into()
                    .map_err(|_| Error::TimeoutTooLarge(timeout.as_millis()))?,
            },
        );
        self.builder.finish(request, None);
        let request_bytes = Bytes::copy_from_slice(self.builder.finished_data());
        let url = format!("{}/v1/html", self.url);
        let resp = self.http
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(request_bytes.to_vec())
            .send()
            .await?;
        resp.bytes().await.map_err(Error::from)
    }

    pub fn reset(&mut self) {
        self.files.clear();
        self.builder.reset();
    }
}

pub async fn main(
    address: SocketAddr,
    language: SharedConfig,
    path: PathBuf,
) -> anyhow::Result<()> {
    // Read file contents
    let contents = std::fs::read(&path)?;
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
        .to_string();

    // Build request using Client
    let mut client = Client::new();
    client.url = format!("http://{}", address);
    client.add_file(0, Some(&filename), &contents, language, false);

    // Send request and get response bytes
    let response_bytes = client.html(Duration::from_secs(30)).await?;

    // Parse FlatBuffers response
    let fb_response = flatbuffers::root::<html::Response>(&response_bytes)?;

    // Process documents
    if let Some(documents) = fb_response.documents() {
        if documents.len() > 0 {
            let doc = documents.get(0);

            // Check for errors
            let error_code = doc.error_code();
            if error_code.0 != 0 {
                anyhow::bail!("Highlighting failed with error code: {:?}", error_code);
            }

            // Write to /tmp/${FILENAME}.html line by line
            let output_path = format!("/tmp/{}.html", filename);
            let mut file = std::fs::File::create(&output_path)?;
            if let Some(lines) = doc.lines() {
                use std::io::Write;
                for i in 0..lines.len() {
                    let line = lines.get(i);
                    file.write_all(line.as_bytes())?;
                }
            }
            println!("Wrote highlighted output to: {}", output_path);
        }
    }

    Ok(())
}
