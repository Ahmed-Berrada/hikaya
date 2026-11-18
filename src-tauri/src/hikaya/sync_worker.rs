//! Runs sync in the background and answers the UI's hub commands.
//!
//! A run starts every few minutes, right after each dictation, and when the
//! user presses "Sync now". Only one run happens at a time.

use super::store::LearningStore;
use super::sync::{sync_once, HttpHub, HubError, HubLockState};
use crate::managers::history::{HistoryEntry, HistoryManager};
use serde::Serialize;
use specta::Type;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

/// Time between background runs when nothing else triggers one.
const INTERVAL: Duration = Duration::from_secs(300);

/// What the last sync attempt found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// No hub is connected.
    NotConnected,
    /// Connected; nothing has been tried yet since the app started.
    Waiting,
    /// A run is in progress.
    Syncing,
    /// The last run succeeded.
    UpToDate,
    /// The hub could not be reached. Changes wait in the queue.
    Offline,
    /// The hub is locked until its passphrase is entered.
    Locked,
    /// The hub no longer accepts this device's token.
    Unauthorized,
    /// Something else failed; see `message`.
    Error,
}

/// Everything the Sync screen shows.
#[derive(Clone, Debug, Serialize, Type)]
pub struct SyncStatus {
    pub state: SyncState,
    pub url: Option<String>,
    pub device_name: Option<String>,
    /// Detail for `error`, `offline` and refusals.
    pub message: Option<String>,
    /// Dictations, rules and corrections waiting to be sent.
    pub pending: u32,
    /// Unix ms of the last successful sync.
    pub last_sync_at: Option<i64>,
}

/// Background sync, registered as Tauri state.
pub struct SyncWorker {
    store: Arc<LearningStore>,
    wake: Notify,
    last: Mutex<(SyncState, Option<String>)>,
    running: tokio::sync::Mutex<()>,
    /// Set in the app (not in tests): lets a good sync look for phone
    /// recordings to transcribe.
    app: Option<AppHandle>,
}

impl SyncWorker {
    pub fn new(store: Arc<LearningStore>) -> Self {
        let state = match store.hub_config() {
            Ok(Some(_)) => SyncState::Waiting,
            _ => SyncState::NotConnected,
        };
        Self {
            store,
            wake: Notify::new(),
            last: Mutex::new((state, None)),
            running: tokio::sync::Mutex::new(()),
            app: None,
        }
    }

    fn set(&self, state: SyncState, message: Option<String>) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = (state, message);
    }

    /// The current status for the UI.
    pub fn status(&self) -> SyncStatus {
        let (state, message) = self.last.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let config = self.store.hub_config().ok().flatten();
        SyncStatus {
            state: if config.is_none() {
                SyncState::NotConnected
            } else {
                state
            },
            url: config.as_ref().map(|c| c.url.clone()),
            device_name: config.map(|c| c.device_name),
            message,
            pending: self.store.pending_count().unwrap_or(0),
            last_sync_at: self.store.last_sync_at().ok().flatten(),
        }
    }

    /// Asks for a run as soon as possible.
    pub fn trigger(&self) {
        self.wake.notify_one();
    }

    /// Runs one sync now, unless one is already running.
    pub async fn run(&self) -> SyncStatus {
        let Ok(_guard) = self.running.try_lock() else {
            return self.status();
        };
        let config = match self.store.hub_config() {
            Ok(Some(config)) => config,
            Ok(None) => {
                self.set(SyncState::NotConnected, None);
                return self.status();
            }
            Err(e) => {
                self.set(SyncState::Error, Some(format!("{e:#}")));
                return self.status();
            }
        };
        self.set(SyncState::Syncing, None);
        let result = match HttpHub::for_config(&config) {
            Ok(hub) => sync_once(&self.store, &hub).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(report) => {
                if report.received_rules > 0 || report.sent_dictations > 0 {
                    log::info!(
                        "Hub sync: sent {} dictation(s) and {} rule change(s), received {} rule change(s)",
                        report.sent_dictations,
                        report.sent_rules,
                        report.received_rules
                    );
                }
                self.set(SyncState::UpToDate, None);
                if let Some(app) = &self.app {
                    super::meeting::phone::check(app, &config);
                }
            }
            Err(error) => {
                log::warn!("Hub sync did not complete: {error}");
                let state = match &error {
                    HubError::Offline(_) => SyncState::Offline,
                    HubError::Locked => SyncState::Locked,
                    HubError::Unauthorized => SyncState::Unauthorized,
                    HubError::Rejected(_) | HubError::Other(_) => SyncState::Error,
                };
                self.set(state, Some(error.to_string()));
            }
        }
        self.status()
    }

    /// The background loop. Never returns.
    pub async fn run_forever(self: Arc<Self>) {
        loop {
            tokio::select! {
                () = self.wake.notified() => {},
                () = tokio::time::sleep(INTERVAL) => {},
            }
            if matches!(self.store.hub_config(), Ok(Some(_))) {
                self.run().await;
            }
        }
    }
}

/// The text of a history entry as it was pasted.
fn pasted_text(entry: &HistoryEntry) -> &str {
    entry
        .post_processed_text
        .as_deref()
        .unwrap_or(&entry.transcription_text)
}

/// Called right after a dictation is saved to history: counts it for the
/// stats panel and queues it for the hub.
pub fn on_dictation_saved(app: &AppHandle, entry: &HistoryEntry) {
    if let Some(store) = app.try_state::<Arc<LearningStore>>() {
        let (today, _) = super::stats::local_week();
        if let Err(e) = store.count_dictation(&today, pasted_text(entry)) {
            log::warn!("Could not count the dictation: {e:#}");
        }
    }
    let Some(worker) = app.try_state::<Arc<SyncWorker>>() else {
        return;
    };
    match worker.store.queue_dictation(
        entry.id,
        entry.timestamp * 1000,
        pasted_text(entry),
        entry.language.as_deref(),
    ) {
        Ok(true) => worker.trigger(),
        Ok(false) => {}
        Err(e) => log::error!("Could not queue the dictation for the hub: {e:#}"),
    }
}

/// Called after the user corrects a dictation in History.
pub fn on_dictation_corrected(app: &AppHandle, entry: &HistoryEntry, corrected: &str) {
    let Some(worker) = app.try_state::<Arc<SyncWorker>>() else {
        return;
    };
    match worker.store.queue_dictation(
        entry.id,
        entry.timestamp * 1000,
        corrected,
        entry.language.as_deref(),
    ) {
        Ok(true) => worker.trigger(),
        Ok(false) => {}
        Err(e) => log::error!("Could not queue the correction for the hub: {e:#}"),
    }
}

/// Queues the whole current history, oldest first, after connecting.
async fn queue_existing_history(
    store: &LearningStore,
    history: &HistoryManager,
) -> anyhow::Result<usize> {
    let mut entries = Vec::new();
    let mut cursor = None;
    loop {
        let page = history.get_history_entries(cursor, Some(200)).await?;
        cursor = page.entries.last().map(|e| e.id);
        entries.extend(page.entries);
        if !page.has_more || cursor.is_none() {
            break;
        }
    }
    entries.reverse();
    let mut queued = 0;
    for entry in &entries {
        if store.queue_dictation(
            entry.id,
            entry.timestamp * 1000,
            pasted_text(entry),
            entry.language.as_deref(),
        )? {
            queued += 1;
        }
    }
    Ok(queued)
}

fn message(error: HubError) -> String {
    match error {
        HubError::Rejected(why) => why,
        other => other.to_string(),
    }
}

/// Connects this device to a hub: creates the hub's vault if it has none,
/// unlocks it if needed, registers the device, then queues the history.
pub async fn connect(
    app: &AppHandle,
    url: &str,
    device_name: &str,
    passphrase: &str,
) -> Result<SyncStatus, String> {
    let worker = app
        .try_state::<Arc<SyncWorker>>()
        .ok_or("sync is unavailable")?
        .inner()
        .clone();
    let device_name = device_name.trim();
    if device_name.is_empty() {
        return Err("give this device a name".into());
    }
    let hub = HttpHub::new(url, None).map_err(message)?;
    match hub.status().await.map_err(message)? {
        HubLockState::Uninitialized => hub.initialize(passphrase).await.map_err(message)?,
        HubLockState::Locked => hub.unlock(passphrase).await.map_err(message)?,
        HubLockState::Unlocked => {}
    }
    let token = hub
        .register(passphrase, device_name)
        .await
        .map_err(message)?;
    let config = worker
        .store
        .save_hub_config(
            &super::sync::normalize_url(url).map_err(message)?,
            &token,
            device_name,
        )
        .map_err(|e| format!("{e:#}"))?;
    log::info!(
        "Connected to the hub at {} as '{}'",
        config.url,
        config.device_name
    );

    if let Some(history) = app.try_state::<Arc<HistoryManager>>() {
        match queue_existing_history(&worker.store, &history).await {
            Ok(count) => log::info!("Queued {count} existing dictation(s) for the hub"),
            Err(e) => log::error!("Could not queue the existing history: {e:#}"),
        }
    }
    worker.set(SyncState::Waiting, None);
    Ok(worker.run().await)
}

/// Unlocks the hub with its passphrase, then syncs.
pub async fn unlock(app: &AppHandle, passphrase: &str) -> Result<SyncStatus, String> {
    let worker = app
        .try_state::<Arc<SyncWorker>>()
        .ok_or("sync is unavailable")?
        .inner()
        .clone();
    let config = worker
        .store
        .hub_config()
        .map_err(|e| format!("{e:#}"))?
        .ok_or("no hub is connected")?;
    HttpHub::new(&config.url, None)
        .map_err(message)?
        .unlock(passphrase)
        .await
        .map_err(message)?;
    Ok(worker.run().await)
}

/// Starts the worker. Call once at startup, after the store is managed.
pub fn start(app: &AppHandle, store: Arc<LearningStore>) {
    let mut worker = SyncWorker::new(store);
    worker.app = Some(app.clone());
    let worker = Arc::new(worker);
    app.manage(worker.clone());
    tauri::async_runtime::spawn(async move {
        // Give startup a moment before the first run.
        tokio::time::sleep(Duration::from_secs(5)).await;
        worker.trigger();
        worker.run_forever().await;
    });
}
