mod errors;

use std::ffi::CString;

use chrono::prelude::*;
use errors::*;
use pcsc::{Card, Context, Protocols, Scope, ShareMode};
use serde::Serialize;

const APDU_SELECT: &[u8] =
    b"\x00\xA4\x04\x00\x10\xD1\x58\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x11\x00";
const APDU_READ: &[u8] = b"\x00\xCA\x11\x00\x02\x00\x00";

#[derive(Debug, Serialize)]
pub enum Sex {
    #[serde(rename = "M")]
    Male,
    #[serde(rename = "F")]
    Female,
}

#[derive(Debug, Serialize)]
pub struct NHICardBasic {
    pub reader_name:          Option<String>,
    pub card_no:              String,
    pub full_name:            String,
    pub id_no:                String,
    pub birth_date:           NaiveDate,
    pub birth_date_timestamp: i64,
    pub sex:                  Sex,
    pub issue_date:           NaiveDate,
    pub issue_date_timestamp: i64,
}

impl NHICardBasic {
    fn raw_to_naive_date(data: &[u8]) -> Result<NaiveDate, NHICardParseError> {
        let s = String::from_utf8(data.to_vec())?;

        let year = {
            let tw_year = s.chars().take(3).collect::<String>().parse::<i32>()?;

            1911 + tw_year
        };

        let month = s.chars().skip(3).take(2).collect::<String>().parse::<u32>()?;
        let date = s.chars().skip(5).take(2).collect::<String>().parse::<u32>()?;

        match NaiveDate::from_ymd_opt(year, month, date) {
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
            },
        };

        let issue_date = Self::raw_to_naive_date(&data[50..57])?;

        Ok(Self {
            reader_name: None,
            card_no,
            full_name,
            id_no,
            birth_date,
            birth_date_timestamp: NaiveDateTime::new(birth_date, NaiveTime::default())
                .and_local_timezone(Local)
                .latest()
                .unwrap()
                .timestamp_millis(),
            sex,
            issue_date,
            issue_date_timestamp: NaiveDateTime::new(birth_date, NaiveTime::default())
                .and_local_timezone(Local)
                .latest()
                .unwrap()
                .timestamp_millis(),
        })
    }
}

#[inline]
pub fn pcsc_ctx() -> Result<Context, pcsc::Error> {
    Context::establish(Scope::User)
}

#[inline]
pub fn list_readers_len(pcsc_ctx: &Context) -> Result<usize, pcsc::Error> {
    pcsc_ctx.list_readers_len()
}

pub fn list_readers(pcsc_ctx: &Context) -> Result<Vec<String>, pcsc::Error> {
    let size = list_readers_len(pcsc_ctx)?.max(2048);

    let mut buffer: Vec<u8> = vec![0u8; size];

    let names = pcsc_ctx.list_readers(&mut buffer)?;

    let mut readers = Vec::with_capacity(1);

    for name in names {
        match name.to_str() {
            Ok(name) => readers.push(String::from(name)),
            Err(err) => {
                tracing::warn!("{err}")
            },
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
        let card = match connect_card(pcsc_ctx, reader.as_str()) {
            Ok(card) => card,
            Err(err) => match err {
                pcsc::Error::NoSmartcard | pcsc::Error::RemovedCard => {
                    continue;
                },
                _ => {
                    return Err(err);
                },
            },
        };

        match get_nhi_data(card) {
            Ok(mut basic) => {
                basic.reader_name = Some(reader.clone());

                output.push(basic);
            },
            Err(err) => match err {
                NHICardError::PCSCError(err) => {
                    return Err(err);
                },
                NHICardError::ParseError(_) => {
                    continue;
                },
            },
        }
    }

    Ok(output)
}
