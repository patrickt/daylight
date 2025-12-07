use clap::Parser;
use daylight::server;

#[derive(Parser)]
#[command(name = "daylight-server")]
#[command(about = "Blazing-fast syntax highlighting RPC server")]
struct Cli {
    address: std::net::SocketAddr,

    #[arg(long, env = "DAYLIGHT_WORKER_THREADS", default_value = "512")]
    threads: usize,

    #[arg(long, env = "DAYLIGHT_DEFAULT_PER_FILE_TIMEOUT_MS", default_value = "30000")]
    default_timeout_ms: u64,

    #[arg(long, env = "DAYLIGHT_MAX_PER_FILE_TIMEOUT_MS", default_value = "60000")]
    max_timeout_ms: u64,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Build runtime with custom blocking thread pool size
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(cli.threads)
        .enable_all()
        .build()?;

    let default_timeout = tokio::time::Duration::from_millis(cli.default_timeout_ms);
    let max_timeout = tokio::time::Duration::from_millis(cli.max_timeout_ms);
    runtime.block_on(server::main(default_timeout, max_timeout, cli.address))
}
