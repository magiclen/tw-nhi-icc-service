use std::{
    error::Error,
    fmt,
    fmt::{Display, Formatter},
    num::ParseIntError,
    string::FromUtf8Error,
};

use chrono::prelude::*;
use serde::Serialize;

#[derive(Debug)]
pub struct NHICardParseError;

impl Display for NHICardParseError {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
