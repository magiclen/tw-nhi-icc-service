use std::{ffi::CString, io, sync::Arc, thread, time::Duration};

use pcsc::{Context, Error, PNP_NOTIFICATION, ReaderState, Scope, State};
use tokio::sync::watch;

use super::{Reader, ReaderStatus, Snapshot, SnapshotStatus, read_card};

/// 等待讀卡機狀態改變的最長時間，也是重新列出讀卡機與重試讀卡的間隔。
const STATUS_CHANGE_TIMEOUT: Duration = Duration::from_secs(1);
/// PC/SC 服務無法使用時，重新建立 context 的間隔。
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// 可以取得最新讀卡狀態的接收端。在第一次掃描完成之前，值為 `None`。
pub type SnapshotReceiver = watch::Receiver<Option<Arc<Snapshot>>>;

type SnapshotSender = watch::Sender<Option<Arc<Snapshot>>>;

/// 監控中的讀卡機。
struct MonitoredReader {
    reader:           Reader,
    /// 上次讀卡時的卡片事件計數。`None` 代表還沒讀過目前這張卡。
    read_event_count: Option<u32>,
    /// 上次讀卡遇到暫時性的錯誤，下一輪要重試。
    retry:            bool,
}

impl MonitoredReader {
    #[inline]
    fn new(name: &CString) -> Self {
        Self {
            reader:           Reader {
                name:   name.to_string_lossy().into_owned(),
                status: ReaderStatus::Empty,
            },
            read_event_count: None,
            retry:            false,
        }
    }

    fn update(&mut self, context: &Context, state: &ReaderState) {
        let event_state = state.event_state();

        // 讀卡機已被移除或無法使用，等下一輪重新列出讀卡機
        if event_state.intersects(State::UNKNOWN | State::UNAVAILABLE | State::IGNORE) {
            return;
        }

        let status = if event_state.contains(State::PRESENT) {
            let event_count = state.event_count();

            if self.read_event_count == Some(event_count) && !self.retry {
                return;
            }

            self.read_event_count = Some(event_count);

            read_card(context, state.name())
        } else {
            self.read_event_count = None;

            ReaderStatus::Empty
        };

        self.retry =
            matches!(status, ReaderStatus::Error(Error::SharingViolation | Error::ResetCard));

        if self.reader.status != status {
            match &status {
                ReaderStatus::Error(error) => {
                    tracing::warn!(target: "card", reader = self.reader.name, ?error);
                },
                _ => {
                    tracing::info!(target: "card", reader = self.reader.name, state = status.state().as_str());
                },
            }

            self.reader.status = status;
        }
    }
}

/// 啟動背景讀卡監控執行緒。
pub fn spawn_card_monitor() -> io::Result<SnapshotReceiver> {
    let (sender, receiver) = watch::channel(None);

    thread::Builder::new().name(String::from("card-monitor")).spawn(move || run(&sender))?;

    Ok(receiver)
}

fn run(sender: &SnapshotSender) {
    loop {
        let error = match Context::establish(Scope::User) {
            Ok(context) => {
                tracing::info!(target: "card", "PC/SC context established");

                monitor(&context, sender)
            },
            Err(error) => error,
        };

        if publish(sender, SnapshotStatus::PcscUnavailable(error)) {
            tracing::warn!(target: "card", ?error, "PC/SC service unavailable");
        }

        thread::sleep(RETRY_INTERVAL);
    }
}

/// 發布新的讀卡狀態。狀態有改變時回傳 `true`。
fn publish(sender: &SnapshotSender, status: SnapshotStatus) -> bool {
    sender.send_if_modified(|snapshot| {
        if snapshot.as_ref().is_some_and(|snapshot| snapshot.status() == &status) {
            false
        } else {
            *snapshot = Some(Arc::new(Snapshot::new(status)));

            true
        }
    })
}

/// 持續監控讀卡機與卡片的狀態，直到 PC/SC context 無法再使用，並回傳原因。
fn monitor(context: &Context, sender: &SnapshotSender) -> Error {
    // `states` 與 `readers` 的索引互相對應
    let mut states: Vec<ReaderState> = Vec::new();
    let mut readers: Vec<MonitoredReader> = Vec::new();

    let mut pnp = Some(ReaderState::new(PNP_NOTIFICATION(), State::UNAWARE));
    let mut unknown_reader_count = 0u32;

    loop {
        let names = match context.list_readers_owned() {
            Ok(names) => names,
            Err(Error::NoReadersAvailable) => Vec::new(),
            Err(error) => return error,
        };

        let mut old_states = states;
        let mut old_readers = readers;

        states = Vec::with_capacity(names.len() + 1);
        readers = Vec::with_capacity(names.len());

        for name in names {
            match old_states.iter().position(|state| state.name() == name.as_c_str()) {
                Some(index) => {
                    states.push(old_states.swap_remove(index));
                    readers.push(old_readers.swap_remove(index));
                },
                None => {
                    readers.push(MonitoredReader::new(&name));
                    states.push(ReaderState::new(name, State::UNAWARE));
                },
            }
        }

        if let Some(pnp) = pnp.take() {
            states.push(pnp);
        }

        let result = if states.is_empty() {
            thread::sleep(STATUS_CHANGE_TIMEOUT);

            Err(Error::Timeout)
        } else {
            context.get_status_change(STATUS_CHANGE_TIMEOUT, &mut states)
        };

        if states.len() > readers.len() {
            pnp = states.pop();
        }

        match result {
            Ok(()) => {
                unknown_reader_count = 0;

                for (state, reader) in states.iter_mut().zip(readers.iter_mut()) {
                    reader.update(context, state);

                    state.sync_current_state();
                }

                if let Some(state) = pnp.as_mut() {
                    // 有些平台不支援 PnP 通知，改為依靠逾時定期重新列出讀卡機
                    if state.event_state().intersects(State::UNKNOWN | State::IGNORE) {
                        tracing::debug!(target: "card", "PnP notification is not supported");

                        pnp = None;
                    } else {
                        state.sync_current_state();
                    }
                }
            },
            Err(Error::Timeout) => {
                unknown_reader_count = 0;

                for (state, reader) in states.iter().zip(readers.iter_mut()) {
                    if reader.retry {
                        reader.update(context, state);
                    }
                }
            },
            Err(Error::UnknownReader) => {
                // 讀卡機可能在列出之後被移除；連續發生時就是平台不支援 PnP 通知
                unknown_reader_count += 1;

                if unknown_reader_count >= 2 {
                    if pnp.take().is_some() {
                        tracing::debug!(target: "card", "PnP notification is not supported");
                    } else {
                        thread::sleep(STATUS_CHANGE_TIMEOUT);
                    }
                }

                continue;
            },
            Err(Error::NoReadersAvailable) => {
                thread::sleep(STATUS_CHANGE_TIMEOUT);

                continue;
            },
            Err(error) => return error,
        }

        publish(
            sender,
            SnapshotStatus::Ok(readers.iter().map(|reader| reader.reader.clone()).collect()),
        );
    }
}
