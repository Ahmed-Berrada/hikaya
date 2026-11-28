//! Hikaya-only features. Everything Hikaya adds to Handy's Rust code lives
//! here, so merges from upstream touch as little as possible.
//!
//! The logic itself is in the `hikaya-core` crate; this module only connects
//! it to the app: storage in the app data directory, the dictation pipeline
//! and the Tauri commands the UI calls.

pub mod cleanup;
pub mod command_mode;
pub mod commands;
pub mod meeting;
pub mod problems;
pub mod snippets;
pub mod stage;
pub mod stats;
pub mod store;
pub mod sync;
pub mod sync_worker;
pub mod target;

use std::sync::{Arc, OnceLock};
use store::LearningStore;

/// The learning store, for code paths that only receive settings (such as
/// post-processing). Set once at startup.
static LEARNING_STORE: OnceLock<Arc<LearningStore>> = OnceLock::new();

/// The learning store, once the app has started.
pub fn learning_store() -> Option<&'static Arc<LearningStore>> {
    LEARNING_STORE.get()
}
use tauri::{AppHandle, Manager};

/// Whether a meeting is being recorded, for the tray.
pub fn meeting_in_progress(app: &AppHandle) -> bool {
    app.try_state::<Arc<meeting::manager::MeetingManager>>()
        .is_some_and(|meetings| meetings.active().is_some())
}

fn is_french(locale: &str) -> bool {
    locale.to_lowercase().starts_with("fr")
}

/// The tray line shown while a meeting records; clicking it stops it.
pub fn tray_meeting_label(locale: &str) -> &'static str {
    if is_french(locale) {
        "● Réunion en cours d'enregistrement — Arrêter"
    } else {
        "● Recording a meeting — Stop"
    }
}

/// Added to the tray tooltip while a meeting records.
pub fn tray_meeting_tooltip(locale: &str) -> &'static str {
    if is_french(locale) {
        "réunion en cours d'enregistrement"
    } else {
        "recording a meeting"
    }
}

/// Starts or stops a meeting, as `--toggle-meeting` asks.
pub fn toggle_meeting(app: &AppHandle) {
    let Some(meetings) = app.try_state::<Arc<meeting::manager::MeetingManager>>() else {
        return;
    };
    let result = if meetings.active().is_some() {
        meetings.stop().map(|_| ())
    } else {
        meetings.start("").map(|_| ())
    };
    if let Err(e) = result {
        log::error!("Could not toggle the meeting: {e:#}");
    }
}

/// Opens the learning database and registers it as Tauri state.
///
/// Fails soft: if the database cannot be opened, dictation still works. There
/// are then no rules, and the learning commands return errors.
pub fn init(app: &AppHandle) {
    let opened = crate::portable::app_data_dir(app)
        .map_err(anyhow::Error::from)
        .and_then(|dir| LearningStore::open(&dir.join("hikaya.db")));
    let store = opened.unwrap_or_else(|err| {
        log::error!("Hikaya learning store unavailable: {err:#}");
        LearningStore::unavailable()
    });
    let store = Arc::new(store);
    let _ = LEARNING_STORE.set(store.clone());
    app.manage(store.clone());
    let meetings = Arc::new(meeting::manager::MeetingManager::new(app, store.clone()));
    if let Err(e) = meetings.prune_audio() {
        log::warn!("Could not remove old meeting audio: {e:#}");
    }
    app.manage(meetings);
    sync_worker::start(app, store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_meeting_text_follows_the_app_language() {
        assert_eq!(
            tray_meeting_label("fr"),
            "● Réunion en cours d'enregistrement — Arrêter"
        );
        assert_eq!(tray_meeting_label("fr-CA"), tray_meeting_label("fr"));
        assert_eq!(tray_meeting_label("en"), "● Recording a meeting — Stop");
        assert_eq!(tray_meeting_label("de"), tray_meeting_label("en"));
        assert_eq!(
            tray_meeting_tooltip("FR"),
            "réunion en cours d'enregistrement"
        );
    }
}
