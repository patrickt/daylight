use clap::Parser;
use daylight::server;
use opentelemetry::global;
use opentelemetry_otlp as otlp;
use opentelemetry_sdk::{metrics, trace};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::{layer::SubscriberExt, Registry};

#[derive(Parser)]
#[command(name = "daylight-server")]
#[command(about = "Blazing-fast syntax highlighting RPC server")]
struct Cli {
    #[arg(short, long, env = "DAYLIGHT_PORT", default_value = "49311")]
    port: u16,

    #[arg(short = 't', long, env = "DAYLIGHT_WORKER_THREADS", default_value = "512")]
    worker_threads: usize,

    #[arg(
        long,
        env = "DAYLIGHT_DEFAULT_PER_FILE_TIMEOUT_MS",
        default_value = "30000"
    )]
    default_timeout_ms: u64,

    #[arg(
        long,
        env = "DAYLIGHT_MAX_PER_FILE_TIMEOUT_MS",
        default_value = "60000"
    )]
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
        .max_blocking_threads(cli.worker_threads)
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let otel_enabled =
            std::env::var("OTEL_SDK_DISABLED").is_ok_and(|s| !s.eq_ignore_ascii_case("true"));
        if otel_enabled {
            let span_exporter = otlp::SpanExporter::builder().with_http().build()?;
            let tracer_provider = trace::SdkTracerProvider::builder()
                .with_simple_exporter(span_exporter)
                .build();
            global::set_tracer_provider(tracer_provider.clone());

            let meter_exporter = otlp::MetricExporter::builder().with_http().build()?;
            let reader = metrics::PeriodicReader::builder(meter_exporter).build();
            let meter_provider = metrics::MeterProviderBuilder::default()
                .with_reader(reader)
                .build();

            let subscriber = Registry::default()
                .with(OpenTelemetryLayer::new(global::tracer("daylight-server")))
                .with(MetricsLayer::new(meter_provider));
            tracing::subscriber::set_global_default(subscriber)?;
        } else {
            let subscriber = tracing_subscriber::fmt()
                .compact()
                .with_file(true)
                .with_line_number(true)
                .with_thread_ids(true)
                .with_target(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)?;
        };

        let default_timeout = tokio::time::Duration::from_millis(cli.default_timeout_ms);
        let max_timeout = tokio::time::Duration::from_millis(cli.max_timeout_ms);
        server::run(cli.port, default_timeout, max_timeout).await
    })
}
