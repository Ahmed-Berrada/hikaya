//! Background failures the user must hear about.
//!
//! Handy's notices take their titles from Handy's translation files; Hikaya
//! keeps its own strings apart (see AGENTS.md). So Hikaya sends one event,
//! and `src/hikaya/problems.ts` shows it with the frontend's `notify`, in
//! the footer until dismissed.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Event the frontend listens to.
pub const EVENT: &str = "hikaya-problem";

/// What went wrong. The frontend maps each to `hikaya.problems.<kind>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// A dictation or correction could not be queued for the hub.
    SyncQueue,
    /// Parts of a meeting could not be transcribed.
    MeetingIncomplete,
    /// A recording from the phone could not be transcribed here.
    PhoneRecording,
}

#[derive(Clone, Debug, Serialize)]
struct Payload {
    kind: Problem,
    /// How many, when it is a count (parts of a meeting).
    count: Option<u64>,
    /// What only Rust knows: an error, a title.
    detail: Option<String>,
}

/// Logs the problem and tells the user.
pub fn report(app: &AppHandle, kind: Problem, count: Option<u64>, detail: impl Into<String>) {
    let detail = detail.into();
    log::warn!("{kind:?}: {detail}");
    let payload = Payload {
        kind,
        count,
        detail: (!detail.is_empty()).then_some(detail),
    };
    if let Err(e) = app.emit(EVENT, payload) {
        log::warn!("Could not show a problem to the user: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_have_the_names_the_frontend_expects() {
        let json = serde_json::to_string(&Payload {
            kind: Problem::MeetingIncomplete,
            count: Some(2),
            detail: None,
        })
        .expect("json");
        assert_eq!(
            json,
            r#"{"kind":"meeting_incomplete","count":2,"detail":null}"#
        );
        assert_eq!(
            serde_json::to_string(&Problem::SyncQueue).expect("json"),
            r#""sync_queue""#
        );
        assert_eq!(
            serde_json::to_string(&Problem::PhoneRecording).expect("json"),
            r#""phone_recording""#
        );
    }
}
