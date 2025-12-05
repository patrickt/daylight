use clap::{Parser, Subcommand};

capnp::generated_code!(pub mod daylight_capnp);

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

        #[arg(
            long,
            env = "DAYLIGHT_HIGHLIGHTER_THREADS",
            default_value = "8"
        )]
        threads: usize,

        #[arg(long, env = "DAYLIGHT_PER_FILE_TIMEOUT_MS", default_value = "30000")]
        timeout_ms: u64,
    },
    /// Run the client
    Client {
        #[arg(short = 'l', long)]
        language: Option<&'static languages::Language>,
        address: std::net::SocketAddr,
        path: std::path::PathBuf,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server {
            address,
            threads,
            timeout_ms,
        } => {
            let timeout = std::time::Duration::from_millis(timeout_ms);
            server::main(threads, timeout, address).await
        }
        Commands::Client {
            language,
            address,
            path,
        } => {
            client::main(
                address,
                language.unwrap_or_else(|| {
                    languages::from_path(&path).expect("Could not infer language from path")
                }),
                path,
            )
            .await
        }
    }
}
