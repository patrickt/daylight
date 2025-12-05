use std::net::SocketAddr;
use std::path::PathBuf;

use crate::languages::Language;

// TODO: Implement Cap'n Proto RPC client

pub async fn main(_address: SocketAddr, _language: &'static Language, _path: PathBuf) -> anyhow::Result<()> {
    println!("Client not yet implemented for Cap'n Proto RPC");
    Ok(())
}
