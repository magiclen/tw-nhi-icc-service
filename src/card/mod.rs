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
    let mut card = match context.connect(reader, ShareMode::Shared, Protocols::ANY) {
        Ok(card) => card,
        Err(Error::NoSmartcard | Error::RemovedCard) => return ReaderStatus::Empty,
        Err(error) => return ReaderStatus::Error(error),
    };

    let status = read_nhi_card(&mut card);

    // `Card` 被 drop 時會重置卡片，所以要用 `LeaveCard` 斷線，避免干擾其他正在使用這張卡片的程式
    if let Err((_, error)) = card.disconnect(Disposition::LeaveCard) {
        tracing::warn!(target: "card", reader = ?reader, ?error);
    }

    status
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
