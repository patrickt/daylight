capnp::generated_code!(pub mod daylight_capnp);

pub mod client;
pub mod server;
pub mod languages;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = ::std::env::args().collect();
    if args.len() >= 2 {
        match &args[1][..] {
            "client" => return client::main().await,
            "server" => return server::main().await,
            _ => (),
        }
    }

    println!("usage: {} [client | server] ADDRESS", args[0]);
    Ok(())
}
