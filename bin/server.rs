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

    // Set default service name for OpenTelemetry if not already configured
    if std::env::var("OTEL_SERVICE_NAME").is_err() {
        unsafe {
            std::env::set_var("OTEL_SERVICE_NAME", "daylight-server");
        }
    }

    // Build runtime with custom blocking thread pool size
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(cli.threads)
        .enable_all()
        .build()?;

    runtime.block_on(async {
        // Initialize OpenTelemetry tracing inside the runtime context
        let otel_disabled = std::env::var("OTEL_SDK_DISABLED")
            .is_ok_and(|v| v.eq_ignore_ascii_case("true"));

        if !otel_disabled {
            init_tracing_opentelemetry::tracing_subscriber_ext::init_subscribers()
                .map_err(|e| anyhow::anyhow!("Failed to initialize tracing: {}", e))?;
        }

        let default_timeout = tokio::time::Duration::from_millis(cli.default_timeout_ms);
        let max_timeout = tokio::time::Duration::from_millis(cli.max_timeout_ms);
        server::run(default_timeout, max_timeout, cli.address).await
    })
}
