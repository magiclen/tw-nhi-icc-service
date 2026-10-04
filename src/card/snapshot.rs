use serde::Serialize;
use utoipa::ToSchema;

use super::NHICardBasic;

/// 讀卡機的狀態。
///
/// - `empty`：沒有插卡。
/// - `nhi_card`：讀到健保卡，資料在 `card` 欄位。
/// - `unsupported_card`：有卡片，但不是健保卡（例如 SAM 卡或晶片金融卡）。
/// - `error`：讀卡失敗，原因在 `error` 欄位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReaderState {
    Empty,
    NhiCard,
    UnsupportedCard,
    Error,
}

impl ReaderState {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NhiCard => "nhi_card",
            Self::UnsupportedCard => "unsupported_card",
            Self::Error => "error",
        }
    }
}

/// 服務的狀態。
///
/// - `ok`：PC/SC 服務可以使用。沒有任何讀卡機時，`readers` 為空陣列。
/// - `pcsc_unavailable`：PC/SC 服務無法使用，原因在 `error` 欄位。服務會自動重試，恢復後就會變回 `ok`。Windows 在沒有接任何讀卡機時，系統的智慧卡服務可能沒有啟動，也會是這個狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Ok,
    PcscUnavailable,
}

/// 訊息的種類，目前只有 `snapshot`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Snapshot,
}

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
    pub const fn state(&self) -> ReaderState {
        match self {
            Self::Empty => ReaderState::Empty,
            Self::NHICard(_) => ReaderState::NhiCard,
            Self::UnsupportedCard => ReaderState::UnsupportedCard,
            Self::Error(_) => ReaderState::Error,
        }
    }
}

/// 一台讀卡機。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub name:   String,
    pub status: ReaderStatus,
}

/// 整個服務的讀卡狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// PC/SC 服務可以使用，包含所有讀卡機的狀態。
    Ok(Vec<Reader>),
    /// PC/SC 服務無法使用。
    PcscUnavailable(pcsc::Error),
}

/// 一台讀卡機。
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = Reader)]
pub struct ReaderJSON {
    /// 讀卡機名稱。
    #[schema(examples("ACS ACR39U ICC Reader 00 00"))]
    name:  String,
    state: ReaderState,
    /// 健保卡的基本資料。只有 `state` 為 `nhi_card` 時才有值，否則為 `null`。
    #[schema(required = true)]
    card:  Option<NHICardBasic>,
    /// PC/SC 的錯誤名稱。只有 `state` 為 `error` 時才有值，否則為 `null`。例如 `SharingViolation` 代表卡片正被其他程式獨占使用，服務會自動重試。
    #[schema(required = true, examples("SharingViolation"))]
    error: Option<String>,
}

impl From<&Reader> for ReaderJSON {
    #[inline]
    fn from(reader: &Reader) -> Self {
        let (card, error) = match &reader.status {
            ReaderStatus::NHICard(card) => (Some(card.clone()), None),
            ReaderStatus::Error(error) => (None, Some(format!("{error:?}"))),
            ReaderStatus::Empty | ReaderStatus::UnsupportedCard => (None, None),
        };

        Self {
            name: reader.name.clone(),
            state: reader.status.state(),
            card,
            error,
        }
    }
}

/// 所有讀卡機目前的狀態。`GET /` 的回應與 WebSocket 的訊息都是這個格式。
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = Snapshot)]
pub struct SnapshotJSON {
    /// 訊息的種類，固定為 `snapshot`。
    #[serde(rename = "type")]
    kind:    MessageType,
    status:  ServiceStatus,
    /// PC/SC 的錯誤名稱。只有 `status` 為 `pcsc_unavailable` 時才有值，否則為 `null`。
    #[schema(required = true, examples("NoService"))]
    error:   Option<String>,
    /// 所有讀卡機。`status` 為 `pcsc_unavailable` 時為空陣列。
    readers: Vec<ReaderJSON>,
}

impl From<&SnapshotStatus> for SnapshotJSON {
    #[inline]
    fn from(status: &SnapshotStatus) -> Self {
        let (status, error, readers) = match status {
            SnapshotStatus::Ok(readers) => {
                (ServiceStatus::Ok, None, readers.iter().map(ReaderJSON::from).collect())
            },
            SnapshotStatus::PcscUnavailable(error) => {
                (ServiceStatus::PcscUnavailable, Some(format!("{error:?}")), Vec::new())
            },
        };

        Self {
            kind: MessageType::Snapshot,
            status,
            error,
            readers,
        }
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
        let json = serde_json::to_string(&SnapshotJSON::from(&status)).unwrap();

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
