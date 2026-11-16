//! Tauri commands for the learning features. All names start with `hikaya_`
//! so they are easy to tell apart from Handy's in the generated bindings.

use super::store::{LearnedSubstitution, LearningStore, RuleProposal, StoredRule};
use crate::managers::history::HistoryManager;
use std::sync::Arc;
use tauri::State;

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
    store
        .record_correction(Some(id), &before, &corrected_text)
        .map_err(text_error)
}
