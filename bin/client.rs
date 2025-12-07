use clap::Parser;
use daylight::{client, languages};

#[derive(Parser)]
#[command(name = "daylight-client")]
#[command(about = "Client for syntax highlighting RPC server")]
struct Cli {
    #[arg(short = 'l', long)]
    language: Option<&'static languages::Config>,

    address: std::net::SocketAddr,

    path: std::path::PathBuf,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Client uses default runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(client::main(
        cli.address,
        cli.language.unwrap_or_else(|| {
            languages::from_path(&cli.path).expect("Could not infer language from path")
        }),
        cli.path,
    ))
}
