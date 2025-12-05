use thiserror::Error;

use crate::daylight_capnp;

pub use crate::daylight_capnp::Language;
pub use reqwest::ClientBuilder;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Cap'n Proto serialization failed: {0}")]
    Capnp(#[from] capnp::Error),
    #[error("Failed to serialize request")]
    SerializationFailed,
}

pub struct FileEntry {
    pub ident: u16,
    pub filename: String,
    pub language: daylight_capnp::Language,
    pub contents: Vec<u8>,
}

pub struct Request {
    files: Vec<FileEntry>,
    timeout_ms: u64,
}

impl Request {
    pub fn new() -> Self {
        Request {
            files: Vec::new(),
            timeout_ms: 0,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    pub fn add_file(
        &mut self,
        ident: u16,
        filename: &str,
        language: daylight_capnp::Language,
        contents: &[u8],
    ) -> &mut Self {
        self.files.push(FileEntry {
            ident,
            filename: filename.to_string(),
            language,
            contents: contents.to_vec(),
        });
        self
    }

    fn build_message(&self) -> capnp::message::TypedBuilder<daylight_capnp::request::Owned> {
        let mut message = capnp::message::TypedBuilder::<daylight_capnp::request::Owned>::new_default();
        let mut req = message.init_root();
        req.set_timeout_ms(self.timeout_ms);

        let mut files = req.init_files(self.files.len() as u32);
        for (i, file) in self.files.iter().enumerate() {
            let mut f = files.reborrow().get(i as u32);
            f.set_ident(file.ident);
            f.set_filename(&file.filename);
            f.set_language(file.language);
            f.set_contents(&file.contents);
        }

        message
    }
}

impl Default for Request {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Response {
    message: capnp::message::TypedReader<capnp::serialize::OwnedSegments, daylight_capnp::response::Owned>,
}

pub struct DocumentIter<'a> {
    reader: capnp::struct_list::Reader<'a, daylight_capnp::document::Owned>,
    index: u32,
}

impl<'a> Iterator for DocumentIter<'a> {
    type Item = daylight_capnp::document::Reader<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.reader.len() {
            let item = self.reader.get(self.index);
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }
}

pub struct FailureIter<'a> {
    reader: capnp::struct_list::Reader<'a, daylight_capnp::failure::Owned>,
    index: u32,
}

impl<'a> Iterator for FailureIter<'a> {
    type Item = daylight_capnp::failure::Reader<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.reader.len() {
            let item = self.reader.get(self.index);
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }
}

impl Response {
    fn new(message: capnp::message::TypedReader<capnp::serialize::OwnedSegments, daylight_capnp::response::Owned>) -> Self {
        Response { message }
    }

    pub fn documents(&self) -> Result<DocumentIter, ClientError> {
        let reader = self.message.get()?.get_documents()?;
        Ok(DocumentIter {
            reader,
            index: 0,
        })
    }

    pub fn failures(&self) -> Result<FailureIter, ClientError> {
        let reader = self.message.get()?.get_failures()?;
        Ok(FailureIter {
            reader,
            index: 0,
        })
    }
}

pub struct Client {
    http_client: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn new(url: String) -> Result<Self, ClientError> {
        Self::with_builder(url, reqwest::ClientBuilder::new())
    }

    pub fn with_builder(url: String, builder: reqwest::ClientBuilder) -> Result<Self, ClientError> {
        Ok(Client {
            http_client: builder.build()?,
            base_url: url,
        })
    }

    pub async fn html(&self, request: &Request) -> Result<Response, ClientError> {
        // Build the Cap'n Proto message
        let message = request.build_message();

        // Serialize the request to Cap'n Proto binary
        let mut buf = Vec::new();
        capnp::serialize::write_message(&mut buf, message.borrow_inner())
            .map_err(|_| ClientError::SerializationFailed)?;

        // Send HTTP POST request
        let response = self
            .http_client
            .post(format!("{}/v1/html", self.base_url))
            .header("Content-Type", "application/octet-stream")
            .body(buf)
            .send()
            .await?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Http(response.error_for_status().unwrap_err()));
        }

        // Read response body
        let body = response.bytes().await?;

        // Deserialize Cap'n Proto response
        let message_reader = capnp::serialize::read_message(
            &mut &body[..],
            capnp::message::ReaderOptions::new(),
        )?;

        Ok(Response::new(capnp::message::TypedReader::new(message_reader)))
    }
}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
