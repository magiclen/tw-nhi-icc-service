use std::error::Error;
use std::ffi::CString;
use std::fmt::{self, Display, Formatter};
use std::num::ParseIntError;
use std::string::FromUtf8Error;

use chrono::NaiveDate;

use serde::Serialize;

use axum::extract::State;
use axum::http::header::{HeaderName, HeaderValue};
use axum::{routing::get, Json, Router};
use tower_http::set_header::SetResponseHeaderLayer;

use pcsc::{Card, Context, Protocols, Scope, ShareMode};

const APDU_SELECT: &[u8] =
    b"\x00\xA4\x04\x00\x10\xD1\x58\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x11\x00";
const APDU_READ: &[u8] = b"\x00\xCA\x11\x00\x02\x00\x00";

#[derive(Debug)]
pub struct NHICardParseError;

impl Display for NHICardParseError {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("不是正確的健保卡")
    }
}

impl Error for NHICardParseError {}

impl From<FromUtf8Error> for NHICardParseError {
    #[inline]
    fn from(_: FromUtf8Error) -> Self {
        NHICardParseError
    }
}

impl From<ParseIntError> for NHICardParseError {
    #[inline]
    fn from(_: ParseIntError) -> Self {
        NHICardParseError
    }
}

#[derive(Debug)]
pub enum NHICardError {
    PCSCError(pcsc::Error),
    ParseError(NHICardParseError),
}

impl Display for NHICardError {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            NHICardError::PCSCError(err) => Display::fmt(&err, f),
            NHICardError::ParseError(err) => Display::fmt(&err, f),
        }
    }
}

impl Error for NHICardError {}

impl From<pcsc::Error> for NHICardError {
    #[inline]
    fn from(err: pcsc::Error) -> Self {
        NHICardError::PCSCError(err)
    }
}

impl From<NHICardParseError> for NHICardError {
    #[inline]
    fn from(err: NHICardParseError) -> Self {
        NHICardError::ParseError(err)
    }
}

#[derive(Debug, Serialize)]
pub enum Sex {
    #[serde(rename = "M")]
    Male,
    #[serde(rename = "F")]
    Female,
}

impl Sex {
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Male => "M",
            Self::Female => "F",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NHICardBasic {
    pub reader_name: Option<String>,
    pub card_no: String,
    pub full_name: String,
    pub id_no: String,
    pub birth_date: NaiveDate,
    pub sex: Sex,
    pub issue_date: NaiveDate,
}

impl NHICardBasic {
    fn raw_to_naive_date(data: &[u8]) -> Result<NaiveDate, NHICardParseError> {
        let s = String::from_utf8(data.to_vec())?;

        let tw_year = {
            let y = s.chars().take(3).collect::<String>().parse::<i32>()?;

            1911 + y
        };

        let month = s.chars().skip(3).take(2).collect::<String>().parse::<u32>()?;
        let date = s.chars().skip(5).take(2).collect::<String>().parse::<u32>()?;

        match NaiveDate::from_ymd_opt(tw_year, month, date) {
            Some(date) => Ok(date),
            None => Err(NHICardParseError),
        }
    }

    pub fn from_raw<D: AsRef<[u8]>>(data: D) -> Result<Self, NHICardParseError> {
        let data = data.as_ref();

        if data.len() < 57 {
            return Err(NHICardParseError);
        }

        let card_no = String::from_utf8(data[..12].to_vec())?;

        let full_name = {
            let s = 12usize;
            let mut e = s;

            while e < 32 {
                if data[e] == 0 {
                    break;
                }

                e += 1;
            }

            let (cow, _encoding_used, had_errors) = encoding_rs::BIG5.decode(&data[s..e]);

            if had_errors {
                return Err(NHICardParseError);
            }

            cow.into_owned()
        };

        let id_no = String::from_utf8(data[32..42].to_vec())?;

        let birth_date = Self::raw_to_naive_date(&data[42..49])?;

        let sex = match data[49] {
            b'M' => Sex::Male,
            b'F' => Sex::Female,
            _ => {
                return Err(NHICardParseError);
            }
        };

        let issue_date = Self::raw_to_naive_date(&data[50..57])?;

        Ok(Self {
            reader_name: None,
            card_no,
            full_name,
            id_no,
            birth_date,
            sex,
            issue_date,
        })
    }
}

#[inline]
pub fn list_readers_len(pcsc_ctx: &Context) -> Result<usize, pcsc::Error> {
    pcsc_ctx.list_readers_len()
}

pub fn list_readers(pcsc_ctx: &Context) -> Result<Vec<String>, pcsc::Error> {
    let size = list_readers_len(pcsc_ctx)?.max(2048);

    let mut buffer: Vec<u8> = Vec::with_capacity(size);

    #[allow(clippy::uninit_vec)]
    unsafe {
        buffer.set_len(size);
    }

    let names = pcsc_ctx.list_readers(&mut buffer)?;

    let mut readers = Vec::with_capacity(1);

    for name in names {
        match name.to_str() {
            Ok(name) => readers.push(String::from(name)),
            Err(err) => {
                tracing::warn!("{err}")
            }
        }
    }

    Ok(readers)
}

#[inline]
pub fn connect_card<S: AsRef<str>>(
    pcsc_ctx: &Context,
    reader_name: S,
) -> Result<Card, pcsc::Error> {
    let reader_name = CString::new(reader_name.as_ref()).map_err(|_| pcsc::Error::UnknownReader)?;
    pcsc_ctx.connect(reader_name.as_c_str(), ShareMode::Shared, Protocols::ANY)
}

pub fn get_nhi_data(card: Card) -> Result<NHICardBasic, NHICardError> {
    let mut buffer = [0u8; 59];

    let result = card.transmit(APDU_SELECT, &mut buffer)?;

    if result != [144, 0] {
        return Err(NHICardParseError.into());
    }

    let result = card.transmit(APDU_READ, &mut buffer)?;

    Ok(NHICardBasic::from_raw(result)?)
}

pub fn read_nhi_cards(pcsc_ctx: &Context) -> Result<Vec<NHICardBasic>, pcsc::Error> {
    let mut output: Vec<NHICardBasic> = Vec::new();

    let readers = list_readers(pcsc_ctx)?;

    for reader in readers {
        let card = connect_card(pcsc_ctx, reader.as_str())?;

        match get_nhi_data(card) {
            Ok(mut basic) => {
                basic.reader_name = Some(reader.clone());

                output.push(basic);
            }
            Err(err) => {
                match err {
                    NHICardError::PCSCError(err) => {
                        return Err(err);
                    }
                    NHICardError::ParseError(_) => {
                        continue;
                    }
                }
            }
        }
    }

    Ok(output)
}

pub async fn index_handler(State(pcsc_ctx): State<Context>) -> Json<Vec<NHICardBasic>> {
    let result = read_nhi_cards(&pcsc_ctx).unwrap();

    Json(result)
}

pub async fn version_handler() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn create_app() -> Result<Router, String> {
    let pcsc_ctx = match Context::establish(Scope::User) {
        Ok(ctx) => ctx,
        Err(err) => {
            return Err(format!("找不到 PC/SC 服務：{err}"));
        }
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/version", get(version_handler))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-store"),
        ))
        .with_state(pcsc_ctx);

    Ok(app)
}
