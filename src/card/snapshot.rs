use serde::{Serialize, Serializer, ser::SerializeStruct};

use super::NHICardBasic;

/// 一台讀卡機目前的狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderStatus {
    /// 沒有插卡。
    Empty,
    /// 讀到健保卡。
    NHICard(NHICardBasic),
    /// 有卡片，但不是健保卡。
    UnsupportedCard,
    /// 讀卡失敗。
    Error(pcsc::Error),
}

impl ReaderStatus {
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NHICard(_) => "nhi_card",
            Self::UnsupportedCard => "unsupported_card",
            Self::Error(_) => "error",
        }
    }
}

/// 一台讀卡機。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub name:   String,
    pub status: ReaderStatus,
}

impl Serialize for Reader {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (card, error) = match &self.status {
            ReaderStatus::NHICard(card) => (Some(card), None),
            ReaderStatus::Error(error) => (None, Some(format!("{error:?}"))),
            ReaderStatus::Empty | ReaderStatus::UnsupportedCard => (None, None),
        };

        let mut s = serializer.serialize_struct("Reader", 4)?;

        s.serialize_field("name", &self.name)?;
        s.serialize_field("state", self.status.as_str())?;
        s.serialize_field("card", &card)?;
        s.serialize_field("error", &error)?;

        s.end()
    }
}

/// 整個服務的讀卡狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// PC/SC 服務可以使用，包含所有讀卡機的狀態。
    Ok(Vec<Reader>),
    /// PC/SC 服務無法使用。
    PcscUnavailable(pcsc::Error),
}

impl Serialize for SnapshotStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (status, error, readers) = match self {
            Self::Ok(readers) => ("ok", None, readers.as_slice()),
            Self::PcscUnavailable(error) => {
                ("pcsc_unavailable", Some(format!("{error:?}")), [].as_slice())
            },
        };

        let mut s = serializer.serialize_struct("Snapshot", 4)?;

        s.serialize_field("type", "snapshot")?;
        s.serialize_field("status", status)?;
        s.serialize_field("error", &error)?;
        s.serialize_field("readers", readers)?;

        s.end()
    }
}

/// 某個時間點的讀卡狀態，並事先序列化成 JSON。
#[derive(Debug)]
pub struct Snapshot {
    status: SnapshotStatus,
    json:   String,
}

impl Snapshot {
    #[inline]
    pub fn new(status: SnapshotStatus) -> Self {
        let json = serde_json::to_string(&status).unwrap();

        Self {
            status,
            json,
        }
    }

    #[inline]
    pub const fn status(&self) -> &SnapshotStatus {
        &self.status
    }

    #[inline]
    pub const fn json(&self) -> &str {
        self.json.as_str()
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use serde_json::{Value, json};

    use super::*;
    use crate::card::Sex;

    #[test]
    fn snapshot_json() {
        let card = NHICardBasic {
            card_no:              String::from("000012345678"),
            full_name:            String::from("王小明"),
            id_no:                String::from("A123456789"),
            birth_date:           NaiveDate::from_ymd_opt(1990, 1, 1).unwrap(),
            birth_date_timestamp: 631123200000,
            sex:                  Sex::Male,
            issue_date:           NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            issue_date_timestamp: 1577808000000,
        };

        let snapshot = Snapshot::new(SnapshotStatus::Ok(vec![
            Reader {
                name: String::from("Reader 0"), status: ReaderStatus::NHICard(card)
            },
            Reader {
                name: String::from("Reader 1"), status: ReaderStatus::Empty
            },
            Reader {
                name: String::from("Reader 2"), status: ReaderStatus::UnsupportedCard
            },
            Reader {
                name:   String::from("Reader 3"),
                status: ReaderStatus::Error(pcsc::Error::SharingViolation),
            },
        ]));

        assert_eq!(
            json!({
                "type": "snapshot",
                "status": "ok",
                "error": null,
                "readers": [
                    {
                        "name": "Reader 0",
                        "state": "nhi_card",
                        "card": {
                            "card_no": "000012345678",
                            "full_name": "王小明",
                            "id_no": "A123456789",
                            "birth_date": "1990-01-01",
                            "birth_date_timestamp": 631123200000i64,
                            "sex": "M",
                            "issue_date": "2020-01-01",
                            "issue_date_timestamp": 1577808000000i64,
                        },
                        "error": null,
                    },
                    { "name": "Reader 1", "state": "empty", "card": null, "error": null },
                    { "name": "Reader 2", "state": "unsupported_card", "card": null, "error": null },
                    { "name": "Reader 3", "state": "error", "card": null, "error": "SharingViolation" },
                ],
            }),
            serde_json::from_str::<Value>(snapshot.json()).unwrap()
        );
    }

    #[test]
    fn snapshot_json_pcsc_unavailable() {
        let snapshot = Snapshot::new(SnapshotStatus::PcscUnavailable(pcsc::Error::NoService));

        assert_eq!(
            json!({
                "type": "snapshot",
                "status": "pcsc_unavailable",
                "error": "NoService",
                "readers": [],
            }),
            serde_json::from_str::<Value>(snapshot.json()).unwrap()
        );
    }
}
