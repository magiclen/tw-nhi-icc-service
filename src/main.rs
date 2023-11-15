mod card;
mod cli;
mod server;

use std::net::SocketAddr;

use cli::*;
use server::*;
use tokio::runtime;

fn main() -> anyhow::Result<()> {
    let args = get_args();

    let socket_addr = SocketAddr::new(args.interface, args.port);

    let runtime = runtime::Runtime::new()?;

    runtime.block_on(async move { server_main(socket_addr).await })
}
