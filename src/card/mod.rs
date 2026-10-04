mod monitor;
mod nhi_card_basic;
mod snapshot;

use std::ffi::CStr;

pub use monitor::*;
pub use nhi_card_basic::*;
use pcsc::{Card, Context, Disposition, Error, MAX_BUFFER_SIZE, Protocols, ShareMode};
pub use snapshot::*;

const APDU_SELECT: &[u8] =
    b"\x00\xA4\x04\x00\x10\xD1\x58\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x11\x00";
const APDU_READ: &[u8] = b"\x00\xCA\x11\x00\x02\x00\x00";

/// 讀取讀卡機中的卡片。
fn read_card(context: &Context, reader: &CStr) -> ReaderStatus {
    match context.connect(reader, ShareMode::Shared, Protocols::ANY) {
        Ok(card) => read_and_disconnect(card, reader),
        Err(Error::NoSmartcard | Error::RemovedCard) => ReaderStatus::Empty,
        Err(error) => ReaderStatus::Error(error),
    }
}

/// 重新讀取應該還插著的卡片，確認卡片沒有被拔出或更換。
fn verify_card(context: &Context, reader: &CStr) -> ReaderStatus {
    match read_card(context, reader) {
        // PC/SC 無法替卡片上電，可能是驅動程式漏掉了拔卡事件，強制重新上電再讀一次
        ReaderStatus::Error(
            Error::ProtoMismatch | Error::UnpoweredCard | Error::UnresponsiveCard,
        ) => (),
        status => return status,
    }

    match repower_card(context, reader) {
        status @ (ReaderStatus::NHICard(_)
        | ReaderStatus::UnsupportedCard
        | ReaderStatus::Error(Error::SharingViolation | Error::ResetCard)) => status,
        // 強制重新上電也失敗，視為卡片已經不在
        _ => ReaderStatus::Empty,
    }
}

/// 強制替卡片重新上電後讀卡。
fn repower_card(context: &Context, reader: &CStr) -> ReaderStatus {
    // Direct 模式在 PC/SC 認為卡片無法使用時也能連線
    let mut card = match context.connect(reader, ShareMode::Direct, Protocols::UNDEFINED) {
        Ok(card) => card,
        Err(error) => return ReaderStatus::Error(error),
    };

    if let Err(error) = card.reconnect(ShareMode::Shared, Protocols::ANY, Disposition::UnpowerCard)
    {
        disconnect(card, reader);

        return ReaderStatus::Error(error);
    }

    read_and_disconnect(card, reader)
}

fn read_and_disconnect(mut card: Card, reader: &CStr) -> ReaderStatus {
    let status = match read_nhi_card(&mut card) {
        // 讀卡途中被拔卡
        ReaderStatus::Error(Error::RemovedCard | Error::NoSmartcard) => ReaderStatus::Empty,
        status => status,
    };

    disconnect(card, reader);

    status
}

/// `Card` 被 drop 時會重置卡片，所以要用 `LeaveCard` 斷線，避免干擾其他正在使用這張卡片的程式。
fn disconnect(card: Card, reader: &CStr) {
    if let Err((_, error)) = card.disconnect(Disposition::LeaveCard) {
        tracing::warn!(target: "card", reader = ?reader, ?error);
    }
}

fn read_nhi_card(card: &mut Card) -> ReaderStatus {
    // 避免其他程式在 SELECT 與 READ 之間對卡片下指令
    let transaction = match card.transaction() {
        Ok(transaction) => transaction,
        Err(error) => return ReaderStatus::Error(error),
    };

    let mut buffer = [0u8; MAX_BUFFER_SIZE];

    match transaction.transmit(APDU_SELECT, &mut buffer) {
        Ok([0x90, 0x00]) => (),
        Ok(_) => return ReaderStatus::UnsupportedCard,
        Err(error) => return ReaderStatus::Error(error),
    }

    let response = match transaction.transmit(APDU_READ, &mut buffer) {
        Ok(response) => response,
        Err(error) => return ReaderStatus::Error(error),
    };

    match response.split_last_chunk() {
        Some((data, [0x90, 0x00])) => match NHICardBasic::from_raw(data) {
            Ok(basic) => ReaderStatus::NHICard(basic),
            Err(_) => ReaderStatus::UnsupportedCard,
        },
        _ => ReaderStatus::UnsupportedCard,
    }
}
