use std::net::IpAddr;

use axum::http::{HeaderValue, Uri};
use clap::{CommandFactory, FromArgMatches, Parser};
use concat_with::concat_line;
use terminal_size::terminal_size;

use crate::server::{MAX_WS_INTERVAL, MIN_WS_INTERVAL};

const APP_NAME: &str = "TW NHI IC Card Service";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const AFTER_HELP: &str = "Enjoy it! https://magiclen.org";

const APP_ABOUT: &str = concat!(
    "透過 HTTP API 讀取中華民國健保卡。\n\nEXAMPLES:\n",
    concat_line!(prefix "tw-nhi-icc-service ",
        "                                         # 啟動 HTTP 服務，監聽 127.0.0.1:12345",
        "-i 0.0.0.0 -p 8080                       # 啟動 HTTP 服務，監聽 0.0.0.0:8080",
        "--allow-origin https://his.example.com   # 只允許 https://his.example.com 的網頁存取",
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
    #[arg(default_value = "127.0.0.1")]
    #[arg(help = "要監聽的網路介面 IP")]
    pub interface: IpAddr,

    #[arg(short, long)]
    #[arg(default_value = "12345")]
    #[arg(help = "要監聽的連接埠")]
    pub port: u16,

    #[arg(long, visible_alias = "interval", value_name = "SECONDS")]
    #[arg(value_parser = clap::value_parser!(u64).range(MIN_WS_INTERVAL..=MAX_WS_INTERVAL))]
    #[arg(default_value = "3")]
    #[arg(help = "WebSocket 在讀卡狀態沒有變化時，重送目前狀態的預設時間間隔（秒）")]
    pub default_ws_card_fetch_interval: u64,

    #[arg(long, value_name = "ORIGIN")]
    #[arg(value_parser = parse_origin)]
    #[arg(
        help = "允許存取此服務的網頁來源（Origin），例如 https://example.com；可重複指定，沒有指定時允許所有來源。有指定時，只能透過 IP 或 localhost 連線到此服務"
    )]
    pub allow_origin: Vec<HeaderValue>,
}

fn parse_origin(arg: &str) -> Result<HeaderValue, String> {
    let uri = arg.parse::<Uri>().map_err(|error| error.to_string())?;

    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return Err(String::from("必須包含協定與主機，例如 https://example.com"));
    };

    if !matches!(uri.path(), "" | "/") || uri.query().is_some() {
        return Err(String::from("不能包含路徑或查詢字串"));
    }

    // 瀏覽器送出的 Origin 不會有結尾的斜線與預設的連接埠，且協定與主機名稱都是小寫
    let scheme = scheme.to_ascii_lowercase();
    let host = authority.host().to_ascii_lowercase();

    let origin = match (scheme.as_str(), authority.port_u16()) {
        ("http", Some(80)) | ("https", Some(443)) | (_, None) => format!("{scheme}://{host}"),
        (_, Some(port)) => format!("{scheme}://{host}:{port}"),
    };

    HeaderValue::from_str(&origin).map_err(|error| error.to_string())
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
