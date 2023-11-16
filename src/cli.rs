use std::{
    net::{AddrParseError, IpAddr},
    str::FromStr,
};

use clap::{CommandFactory, FromArgMatches, Parser};
use concat_with::concat_line;
use terminal_size::terminal_size;

const APP_NAME: &str = "TW NHI IC Card Service";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const AFTER_HELP: &str = "Enjoy it! https://magiclen.org";

const APP_ABOUT: &str = concat!(
    "透過 HTTP API 讀取中華民國健保卡。\n\nEXAMPLES:\n",
    concat_line!(prefix "tw-nhi-icc-service ",
        "                      # 啟動 HTTP 服務，監聽 127.0.0.1:58113",
        "-i 0.0.0.0 -p 12345   # 啟動 HTTP 服務，監聽 0.0.0.0:12345",
    )
);

#[derive(Debug, Parser)]
#[command(name = APP_NAME)]
#[command(term_width = terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))]
#[command(version = CARGO_PKG_VERSION)]
#[command(author = CARGO_PKG_AUTHORS)]
#[command(after_help = AFTER_HELP)]
pub struct CLIArgs {
    #[arg(short, long, visible_alias = "ip")]
    #[arg(value_parser = parse_ip_addr)]
    #[arg(default_value = "127.0.0.1")]
    #[arg(help = "要監聽的網路介面 IP")]
    pub interface: IpAddr,

    #[arg(short, long)]
    #[arg(default_value = "8000")]
    #[arg(help = "要監聽的連接埠")]
    pub port: u16,

    #[arg(long, visible_alias = "interval", value_name = "MILLI_SECONDS")]
    #[arg(default_value = "3000")]
    #[arg(help = "WebSocket 回傳卡片資料的預設時間間隔（毫秒）")]
    pub default_ws_card_fetch_interval: u64,
}

#[inline]
fn parse_ip_addr(arg: &str) -> Result<IpAddr, AddrParseError> {
    IpAddr::from_str(arg)
}

pub fn get_args() -> CLIArgs {
    let args = CLIArgs::command();

    let about = format!("{APP_NAME} {CARGO_PKG_VERSION}\n{CARGO_PKG_AUTHORS}\n{APP_ABOUT}");

    let args = args.about(about);

    let matches = args.get_matches();

    match CLIArgs::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => {
            err.exit();
        },
    }
}
