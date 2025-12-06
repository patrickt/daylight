use clap::{Parser, Subcommand};

#[path = "generated/daylight_generated.rs"]
#[allow(warnings)]
pub mod daylight_generated;

pub mod client;
pub mod languages;
pub mod server;

#[derive(Parser)]
#[command(name = "daylight")]
#[command(about = "Blazing-fast syntax highlighting RPC server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Server {
        address: std::net::SocketAddr,

        #[arg(long, env = "DAYLIGHT_WORKER_THREADS", default_value = "512")]
        threads: usize,

        #[arg(long, env = "DAYLIGHT_DEFAULT_PER_FILE_TIMEOUT_MS", default_value = "30000")]
        default_timeout_ms: u64,

        #[arg(long, env = "DAYLIGHT_MAX_PER_FILE_TIMEOUT_MS", default_value = "60000")]
        max_timeout_ms: u64,
    },
    /// Run the client
    Client {
        #[arg(short = 'l', long)]
        language: Option<&'static languages::Config>,
        address: std::net::SocketAddr,
        path: std::path::PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server {
            address,
            threads,
            default_timeout_ms,
            max_timeout_ms,
        } => {
            // Build runtime with custom blocking thread pool size
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .max_blocking_threads(threads)
                .enable_all()
                .build()?;

            let default_timeout = tokio::time::Duration::from_millis(default_timeout_ms);
            let max_timeout = tokio::time::Duration::from_millis(max_timeout_ms);
            runtime.block_on(server::main(default_timeout, max_timeout, address))
        }
        Commands::Client {
            language,
            address,
            path,
        } => {
            // Client uses default runtime
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;

            runtime.block_on(client::main(
                address,
                language.unwrap_or_else(|| {
                    languages::from_path(&path).expect("Could not infer language from path")
                }),
                path,
            ))
        }
    }
}
