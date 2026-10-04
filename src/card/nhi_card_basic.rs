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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Sex {
    #[serde(rename = "M")]
    Male,
    #[serde(rename = "F")]
    Female,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NHICardBasic {
    pub card_no:              String,
    pub full_name:            String,
    pub id_no:                String,
    pub birth_date:           NaiveDate,
    pub birth_date_timestamp: i64,
    pub sex:                  Sex,
    pub issue_date:           NaiveDate,
    pub issue_date_timestamp: i64,
}

/// 台灣時區（UTC+8）與 UTC 的差距（毫秒）。
const TW_UTC_OFFSET_MILLIS: i64 = 8 * 60 * 60 * 1000;

impl NHICardBasic {
    /// 健保卡上的日期是台灣日期，所以固定以 UTC+8 的午夜計算時間戳記，不受伺服器時區與歷史日光節約時間影響。
    fn naive_date_to_timestamp_millis(date: NaiveDate) -> i64 {
        date.and_time(NaiveTime::MIN).and_utc().timestamp_millis() - TW_UTC_OFFSET_MILLIS
    }

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

            let (full_name, had_errors) =
                encoding_rs::BIG5.decode_without_bom_handling(&data[s..e]);

            // 罕用字可能無法以 Big5 解碼，此時以 U+FFFD 取代，不要讓整張卡無法讀取
            if had_errors {
                tracing::warn!(target: "card", "the full name contains characters that cannot be decoded");
            }

            full_name.into_owned()
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
            card_no,
            full_name,
            id_no,
            birth_date,
            birth_date_timestamp: Self::naive_date_to_timestamp_millis(birth_date),
            sex,
            issue_date,
            issue_date_timestamp: Self::naive_date_to_timestamp_millis(issue_date),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(birth_date: &[u8], issue_date: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(57);

        data.extend_from_slice(b"000012345678");

        let (full_name, ..) = encoding_rs::BIG5.encode("王小明");

        data.extend_from_slice(&full_name);
        data.resize(32, 0);
        data.extend_from_slice(b"A123456789");
        data.extend_from_slice(birth_date);
        data.push(b'M');
        data.extend_from_slice(issue_date);

        data
    }

    #[test]
    fn from_raw() {
        let basic = NHICardBasic::from_raw(raw(b"0790101", b"1090101")).unwrap();

        assert_eq!("000012345678", basic.card_no);
        assert_eq!("王小明", basic.full_name);
        assert_eq!("A123456789", basic.id_no);
        assert_eq!(NaiveDate::from_ymd_opt(1990, 1, 1).unwrap(), basic.birth_date);
        assert_eq!(631123200000, basic.birth_date_timestamp);
        assert_eq!(Sex::Male, basic.sex);
        assert_eq!(NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(), basic.issue_date);
        assert_eq!(1577808000000, basic.issue_date_timestamp);
    }

    #[test]
    fn from_raw_dst_date() {
        // 1955-04-01 是 Asia/Taipei 時區日光節約時間的開始日，時間戳記仍須是 UTC+8 的午夜
        let basic = NHICardBasic::from_raw(raw(b"0440401", b"1040630")).unwrap();

        assert_eq!(NaiveDate::from_ymd_opt(1955, 4, 1).unwrap(), basic.birth_date);
        assert_eq!(-465638400000, basic.birth_date_timestamp);
        assert_eq!(NaiveDate::from_ymd_opt(2015, 6, 30).unwrap(), basic.issue_date);
        assert_eq!(1435593600000, basic.issue_date_timestamp);
    }
}
