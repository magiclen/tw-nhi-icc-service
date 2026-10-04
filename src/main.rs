mod card;
mod cli;
mod server;

use std::net::SocketAddr;

use cli::*;
use server::*;
use tokio::runtime;

fn main() -> anyhow::Result<()> {
    let args = get_args();

    let config = ServerConfig {
        socket_addr:                 SocketAddr::new(args.interface, args.port),
        default_card_fetch_interval: args.default_ws_card_fetch_interval,
    };

    let runtime = runtime::Runtime::new()?;

    runtime.block_on(server_main(config))
}
