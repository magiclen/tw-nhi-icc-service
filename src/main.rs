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
        socket_addr:         SocketAddr::new(args.interface, args.port),
        default_ws_interval: args.default_ws_card_fetch_interval,
        allowed_origins:     args.allow_origin,
    };

    let runtime = runtime::Runtime::new()?;

    runtime.block_on(server_main(config))
}
