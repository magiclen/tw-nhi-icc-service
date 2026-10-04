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

/// 某個時間點的讀卡狀態。
#[derive(Debug)]
pub struct Snapshot {
    status: SnapshotStatus,
}

impl Snapshot {
    #[inline]
    pub const fn new(status: SnapshotStatus) -> Self {
        Self {
            status,
        }
    }

    #[inline]
    pub const fn status(&self) -> &SnapshotStatus {
        &self.status
    }
}
