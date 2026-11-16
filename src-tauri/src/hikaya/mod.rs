//! Hikaya-only features. Everything Hikaya adds to Handy's Rust code lives
//! here, so merges from upstream touch as little as possible.
//!
//! The logic itself is in the `hikaya-core` crate; this module only connects
//! it to the app: storage in the app data directory, the dictation pipeline
//! and the Tauri commands the UI calls.

pub mod cleanup;
pub mod commands;
pub mod stage;
pub mod store;
pub mod sync;

use std::sync::Arc;
use store::LearningStore;
use tauri::{AppHandle, Manager};

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
    app.manage(Arc::new(store));
}
