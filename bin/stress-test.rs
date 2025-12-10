use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use daylight::daylight_generated::daylight::common;
use daylight::daylight_generated::daylight::html::*;

#[derive(Parser)]
#[command(name = "daylight-stress-test")]
#[command(about = "Stress test the Daylight server with multiple files")]
struct Args {
    /// Server address to connect to (e.g., 127.0.0.1:3000)
    #[arg(value_name = "ADDRESS")]
    address: SocketAddr,

    /// Glob pattern to match files (e.g., "**/*.rs", "src/**/*.c")
    #[arg(value_name = "PATTERN")]
    pattern: String,

    /// Whether or not to include other injected languages (better output, slower operation)
    #[arg(short, long, default_value = "false")]
    include_injections: bool,

    /// Timeout per file in milliseconds (0 = use server default)
    #[arg(short, long, default_value = "0")]
    timeout_ms: u64,
}

fn collect_files(pattern: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for entry in glob::glob(pattern)? {
        match entry {
            Ok(path) if path.is_file() => files.push(path),
            Ok(_) => {} // Skip directories
            Err(e) => eprintln!("Error reading entry: {}", e),
        }
    }

    Ok(files)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Collect all files matching the pattern
    println!("Collecting files matching pattern: {}", args.pattern);
    let file_paths = collect_files(&args.pattern)?;

    if file_paths.is_empty() {
        anyhow::bail!("No files found matching pattern: {}", args.pattern);
    }

    println!("Found {} files", file_paths.len());

    // Build FlatBuffers request
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024 * 1024);
    let mut fb_files = Vec::new();

    for (idx, path) in file_paths.iter().enumerate() {
        // Read file contents
        let contents = match std::fs::read(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read {}: {}", path.display(), e);
                continue;
            }
        };

        let filename = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid filename: {}", path.display()))?;

        let filename_offset = builder.create_string(filename);
        let contents_offset = builder.create_vector(&contents);

        let file = File::create(
            &mut builder,
            &FileArgs {
                ident: idx as u16,
                filename: Some(filename_offset),
                contents: Some(contents_offset),
                options: None,
                language: common::Language::Unspecified, // Auto-detect from extension
                include_injections: args.include_injections,
            },
        );

        fb_files.push(file);
    }

    let files_vec = builder.create_vector(&fb_files);
    let request = Request::create(
        &mut builder,
        &RequestArgs {
            files: Some(files_vec),
            timeout_ms: args.timeout_ms,
        },
    );

    builder.finish(request, None);
    let request_bytes = builder.finished_data();

    println!("Sending request with {} files ({} bytes)", fb_files.len(), request_bytes.len());

    // Send HTTP request
    let url = format!("http://{}/v1/html", args.address);
    let client = reqwest::Client::new();

    let start = std::time::Instant::now();
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
    let elapsed = start.elapsed();

    // Parse FlatBuffers response
    let fb_response = flatbuffers::root::<Response>(&response_bytes)?;

    // Count results
    let mut success_count = 0;
    let mut timeout_count = 0;
    let mut unknown_language_count = 0;
    let mut unknown_error_count = 0;

    if let Some(documents) = fb_response.documents() {
        for i in 0..documents.len() {
            let doc = documents.get(i);
            match doc.error_code() {
                common::ErrorCode::NoError => success_count += 1,
                common::ErrorCode::TimedOut => timeout_count += 1,
                common::ErrorCode::UnknownLanguage => unknown_language_count += 1,
                common::ErrorCode::UnknownError => unknown_error_count += 1,
                _ => unknown_error_count += 1,
            }
        }
    }

    let total = success_count + timeout_count + unknown_language_count + unknown_error_count;

    // Print results
    println!("\n=== Stress Test Results ===");
    println!("Total files:          {}", total);
    println!("Successful:           {} ({:.1}%)", success_count, (success_count as f64 / total as f64) * 100.0);
    println!("Failed (timeout):     {}", timeout_count);
    println!("Failed (unknown lang): {}", unknown_language_count);
    println!("Failed (other):       {}", unknown_error_count);
    println!("Time elapsed:         {:?}", elapsed);
    println!("Throughput:           {:.1} files/sec", total as f64 / elapsed.as_secs_f64());

    let failed_count = timeout_count + unknown_language_count + unknown_error_count;
    if failed_count > 0 {
        println!("\nTotal failures:       {}", failed_count);
        std::process::exit(1);
    }

    Ok(())
}
