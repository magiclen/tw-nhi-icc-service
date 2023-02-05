use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::str::FromStr;

use clap::{CommandFactory, FromArgMatches, Parser};
use terminal_size::terminal_size;

use concat_with::concat_line;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use tw_nhi_service::create_app;

const APP_NAME: &str = "TW NHI Card Service";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const APP_ABOUT: &str = concat!(
    "透過 HTTP API 讀取中華民國健保卡。\n\nEXAMPLES:\n",
    concat_line!(prefix "tw-nhi-service ",
        "                     # 啟動 HTTP 服務，監聽 127.0.0.1:58113",
        "-i 0.0.0.0 -p 12345  # 啟動 HTTP 服務，監聽 0.0.0.0:12345",
    )
);

const PORT: u16 = 58113;

#[derive(Debug, Parser)]
#[command(name = APP_NAME)]
#[command(term_width = terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))]
#[command(version = CARGO_PKG_VERSION)]
#[command(author = CARGO_PKG_AUTHORS)]
#[command(after_help = "Enjoy it! https://magiclen.org")]
struct Args {
    /// 要監聽的網路介面 IP
    #[arg(short, long)]
    #[arg(visible_aliases = ["ip"])]
    #[arg(default_value_t = String::from("127.0.0.1"))]
    interface: String,

    /// 要監聽的連接埠
    #[arg(short, long)]
    #[arg(default_value_t = PORT)]
    port: u16,
}

impl Args {
    #[inline]
    pub fn get_listening_addr(&self) -> Result<SocketAddr, AddrParseError> {
        Ok(SocketAddr::new(IpAddr::from_str(self.interface.as_str())?, self.port))
    }
}

#[tokio::main]
async fn main() {
    let mut ansi_color = atty::is(atty::Stream::Stdout);

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let args = get_args();

    match args.get_listening_addr() {
        Ok(addr) => {
            match create_app() {
                Ok(app) => {
                    let app = app.layer(
                        TraceLayer::new_for_http()
                            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                            .on_request(DefaultOnRequest::new().level(Level::INFO))
                            .on_response(DefaultOnResponse::new().level(Level::INFO)),
                    );

                    tracing::info!("listening on {addr}");

                    axum::Server::bind(&addr).serve(app.into_make_service()).await.unwrap();
                }
                Err(err) => {
                    eprintln!("{err}");
                }
            }
        }
        Err(_) => {
            eprintln!("{:?} 不是正確的 IP", args.interface);
        }
    }
}

fn get_args() -> Args {
    let args = Args::command();

    let about = format!("{APP_NAME} {CARGO_PKG_VERSION}\n{CARGO_PKG_AUTHORS}\n{APP_ABOUT}");

    let args = args.about(about);

    let matches = args.get_matches();

    match Args::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => {
            err.exit();
        }
    }
}
