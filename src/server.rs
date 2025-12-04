use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

use futures::AsyncReadExt;
use std::net::ToSocketAddrs;
use std::rc::Rc;

use crate::daylight_capnp::html_highlighter;

struct Daylight {}

impl html_highlighter::Server for Daylight {
    async fn html(
        self: Rc<Self>,
        _params: html_highlighter::HtmlParams,
        _results: html_highlighter::HtmlResults,
    ) -> Result<(), capnp::Error> {
        Ok(())
    }
}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        println!("usage: {} server ADDRESS[:PORT]", args[0]);
        return Ok(());
    }

    let addr = args[2]
        .to_socket_addrs()?
        .next()
        .expect("could not parse address");

    tokio::task::LocalSet::new()
        .run_until(async move {
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let daylight = Daylight{};
            let daylight_client: html_highlighter::Client = capnp_rpc::new_client(daylight);

            loop {
                let (stream, _) = listener.accept().await?;
                stream.set_nodelay(true)?;
                let (reader, writer) =
                    tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
                let network = twoparty::VatNetwork::new(
                    futures::io::BufReader::new(reader),
                    futures::io::BufWriter::new(writer),
                    rpc_twoparty_capnp::Side::Server,
                    Default::default(),
                );

                let rpc_system =
                    RpcSystem::new(Box::new(network), Some(daylight_client.clone().client));

                tokio::task::spawn_local(rpc_system);
            }
        })
        .await
}
