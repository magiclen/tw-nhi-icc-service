mod nhi_card_basic;

use std::marker::PhantomData;

pub use nhi_card_basic::*;
use once_cell::sync::Lazy;
use pcsc::{Context, Protocols, Scope, ShareMode};
use tokio::{sync::Mutex, task};

const APDU_SELECT: &[u8] =
    b"\x00\xA4\x04\x00\x10\xD1\x58\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x11\x00";
const APDU_READ: &[u8] = b"\x00\xCA\x11\x00\x02\x00\x00";

pub static mut CONTEXT: Option<Context> = None;
static mut NHI_CARD_LIST: Vec<NHICardBasic> = Vec::new();
static LOCK: Lazy<Mutex<PhantomData<bool>>> = Lazy::new(|| Mutex::new(PhantomData));
static LOCK_GET: Lazy<Mutex<PhantomData<bool>>> = Lazy::new(|| Mutex::new(PhantomData));

unsafe fn update_nhi_cards(retry: bool) -> Result<(), pcsc::Error> {
    debug_assert!(LOCK.try_lock().is_err());

    if retry {
        tracing::info!(target: "card", "try to re-establish card context");

        CONTEXT = Some(Context::establish(Scope::User)?);
    } else {
        NHI_CARD_LIST.clear();
    }

    let context = CONTEXT.as_ref().unwrap();

    let size = match context.list_readers_len() {
        Ok(len) => len.max(4096),
        Err(error) => {
            if retry {
                return Err(error);
            } else {
                return update_nhi_cards(true);
            }
        },
    };

    let mut buffer: Vec<u8> = vec![0u8; size];

    let names = match context.list_readers(&mut buffer) {
        Ok(names) => names,
        Err(error) => {
            if retry {
                return Err(error);
            } else {
                return update_nhi_cards(true);
            }
        },
    };

    let (readers, readers_cs) = {
        let mut v = Vec::with_capacity(1);
        let mut v_cs = Vec::with_capacity(1);

        for name in names {
            v.push(name.to_string_lossy().into_owned());
            v_cs.push(name);
        }

        (v, v_cs)
    };

    let mut buffer = [0u8; 59];

    for (reader, reader_cs) in readers.into_iter().zip(readers_cs) {
        let card = match context.connect(reader_cs, ShareMode::Shared, Protocols::ANY) {
            Ok(card) => card,
            Err(pcsc::Error::NoSmartcard | pcsc::Error::RemovedCard) => {
                continue;
            },
            Err(error) => {
                tracing::warn!(target: "card", reader, ?error);

                continue;
            },
        };

        match card.transmit(APDU_SELECT, &mut buffer) {
            Ok([144, 0]) => {
                // pass
            },
            Ok(_) => {
                tracing::warn!(target: "card", reader, "unsupported reader");

                continue;
            },
            Err(error) => {
                tracing::warn!(target: "card", reader, ?error);

                continue;
            },
        }

        match card.transmit(APDU_READ, &mut buffer) {
            Ok(result) => match NHICardBasic::from_raw(result) {
                Ok(mut basic) => {
                    basic.reader_name = Some(reader.clone());

                    NHI_CARD_LIST.push(basic);
                },
                Err(error) => {
                    tracing::warn!(target: "card", reader, ?error);

                    continue;
                },
            },
            Err(error) => {
                tracing::warn!(target: "card", reader, ?error);

                continue;
            },
        }
    }

    Ok(())
}

pub async fn fetch_nhi_cards_json_string() -> Result<String, pcsc::Error> {
    let lock_get = LOCK_GET.lock().await;
    let lock_result = LOCK.try_lock();

    drop(lock_get);

    match lock_result {
        Ok(lock) => {
            let lock = unsafe {
                if CONTEXT.is_none() {
                    CONTEXT = Some(Context::establish(Scope::User)?);
                }

                // Move the lock to the synchronized block to prevent the lock being released when executing the synchronized block and the HTTP connection is being disconnected.
                task::spawn_blocking(move || update_nhi_cards(false).map(|_| lock))
                    .await
                    .unwrap()?
            };

            let json = serde_json::to_string(unsafe { &NHI_CARD_LIST }).unwrap();

            drop(lock);

            Ok(json)
        },
        Err(_) => Ok(get_nhi_cards_json_string().await),
    }
}

#[inline]
pub async fn get_nhi_cards_json_string() -> String {
    let lock_get = LOCK_GET.lock().await;
    let lock = LOCK.lock().await;

    let json = serde_json::to_string(unsafe { &NHI_CARD_LIST }).unwrap();

    drop(lock);
    drop(lock_get);

    json
}
