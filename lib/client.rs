use std::net::SocketAddr;
use std::path::PathBuf;

use crate::languages::Config;
use crate::daylight_generated::daylight::html::*;

pub async fn main(address: SocketAddr, language: &'static Config, path: PathBuf) -> anyhow::Result<()> {
    // Read file contents
    let contents = std::fs::read(&path)?;
    let filename = path.file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
        .to_string();

    // Build FlatBuffers request
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(contents.len() + 1024);

    let filename_offset = builder.create_string(&filename);
    let contents_offset = builder.create_vector(&contents);

    let file = File::create(
        &mut builder,
        &FileArgs {
            ident: 0,
            filename: Some(filename_offset),
            contents: Some(contents_offset),
            options: None,
            language: language.fb_language,
            include_injections: false,
        },
    );

    let files_vec = builder.create_vector(&[file]);

    let request = Request::create(
        &mut builder,
        &RequestArgs {
            files: Some(files_vec),
            timeout_ms: 0,
        },
    );

    builder.finish(request, None);
    let request_bytes = builder.finished_data();

    // Send HTTP request
    let url = format!("http://{}/v1/html", address);
    let client = reqwest::Client::new();

    let response = client
        .post(&url)
        .header("Content-Type", "application/octet-stream")
        .body(request_bytes.to_vec())
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Server returned error: {}", response.status());
    }

    let response_bytes = response.bytes().await?;

    // Parse FlatBuffers response
    let fb_response = flatbuffers::root::<Response>(&response_bytes)?;

    // Process documents
    if let Some(documents) = fb_response.documents() {
        if documents.len() > 0 {
            let doc = documents.get(0);

            // Check for errors
            let error_code = doc.error_code();
            if error_code.0 != 0 {  // NoError = 0
                anyhow::bail!("Highlighting failed with error code: {:?}", error_code);
            }

            // Collect all lines into a single string
            let mut html_content = String::new();
            if let Some(lines) = doc.lines() {
                for i in 0..lines.len() {
                    let line = lines.get(i);
                    html_content.push_str(line);
                    html_content.push('\n');
                }
            }

            // Write to /tmp/${FILENAME}.html
            let output_path = format!("/tmp/{}.html", filename);
            std::fs::write(&output_path, html_content)?;
            println!("Wrote highlighted output to: {}", output_path);
        }
    }

    Ok(())
}
