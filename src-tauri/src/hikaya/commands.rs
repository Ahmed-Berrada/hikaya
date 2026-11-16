//! Tauri commands for the learning features. All names start with `hikaya_`
//! so they are easy to tell apart from Handy's in the generated bindings.

use super::store::{LearnedSubstitution, LearningStore, RuleProposal, StoredRule};
use super::sync_worker::{self, SyncStatus, SyncWorker};
use crate::managers::history::HistoryManager;
use std::sync::Arc;
use tauri::{AppHandle, State};

type Store<'a> = State<'a, Arc<LearningStore>>;

fn text_error(err: anyhow::Error) -> String {
    format!("{err:#}")
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_list_rules(store: Store<'_>) -> Result<Vec<StoredRule>, String> {
    store.list_rules().map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_add_rule(
    store: Store<'_>,
    pattern: String,
    replacement: String,
) -> Result<StoredRule, String> {
    store
        .add_rule(&pattern, &replacement, "manual")
        .map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_set_rule_enabled(store: Store<'_>, id: i64, enabled: bool) -> Result<(), String> {
    store.set_rule_enabled(id, enabled).map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_delete_rule(store: Store<'_>, id: i64) -> Result<(), String> {
    store.delete_rule(id).map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_list_proposals(store: Store<'_>) -> Result<Vec<RuleProposal>, String> {
    store.proposals().map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_accept_proposal(
    store: Store<'_>,
    pattern: String,
    replacement: String,
) -> Result<StoredRule, String> {
    store
        .accept_proposal(&pattern, &replacement)
        .map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_dismiss_proposal(
    store: Store<'_>,
    pattern: String,
    replacement: String,
) -> Result<(), String> {
    store
        .dismiss_proposal(&pattern, &replacement)
        .map_err(text_error)
}

/// Saves the user's corrected text for a history entry and learns from the
/// difference. Returns the word replacements that were recorded.
#[tauri::command]
#[specta::specta]
pub async fn hikaya_correct_history_entry(
    app: AppHandle,
    store: Store<'_>,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
    corrected_text: String,
) -> Result<Vec<LearnedSubstitution>, String> {
    let corrected_text = corrected_text.trim().to_string();
    if corrected_text.is_empty() {
        return Err("the corrected text is empty".to_string());
    }
    let before = history_manager
        .hikaya_replace_entry_text(id, &corrected_text)
        .await
        .map_err(text_error)?;
    let learned = store
        .record_correction(Some(id), &before, &corrected_text)
        .map_err(text_error)?;
    if let Ok(Some(entry)) = history_manager.get_entry_by_id(id).await {
        sync_worker::on_dictation_corrected(&app, &entry, &corrected_text);
    }
    Ok(learned)
}

/// Where sync stands, for the Sync screen.
#[tauri::command]
#[specta::specta]
pub fn hikaya_hub_status(worker: State<'_, Arc<SyncWorker>>) -> SyncStatus {
    worker.status()
}

/// Connects this device to a hub with the hub's passphrase. A new hub is set
/// up with that passphrase; a locked one is unlocked.
#[tauri::command]
#[specta::specta]
pub async fn hikaya_hub_connect(
    app: AppHandle,
    url: String,
    device_name: String,
    passphrase: String,
) -> Result<SyncStatus, String> {
    sync_worker::connect(&app, &url, &device_name, &passphrase).await
}

/// Unlocks the connected hub after it restarted.
#[tauri::command]
#[specta::specta]
pub async fn hikaya_hub_unlock(app: AppHandle, passphrase: String) -> Result<SyncStatus, String> {
    sync_worker::unlock(&app, &passphrase).await
}

/// Syncs now instead of waiting for the next run.
#[tauri::command]
#[specta::specta]
pub async fn hikaya_hub_sync_now(worker: State<'_, Arc<SyncWorker>>) -> Result<SyncStatus, String> {
    Ok(worker.inner().clone().run().await)
}

/// Disconnects from the hub. Data already on the hub stays there.
#[tauri::command]
#[specta::specta]
pub fn hikaya_hub_disconnect(
    store: Store<'_>,
    worker: State<'_, Arc<SyncWorker>>,
) -> Result<SyncStatus, String> {
    store.clear_hub_config().map_err(text_error)?;
    Ok(worker.status())
}

/// Usage numbers for the Learning screen.
#[tauri::command]
#[specta::specta]
pub fn hikaya_stats(store: Store<'_>) -> Result<super::stats::Stats, String> {
    let (today, week_start) = super::stats::local_week();
    store.stats(&today, &week_start).map_err(text_error)
}
