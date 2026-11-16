//! What devices and the hub exchange when they sync.
//!
//! Every record has an id chosen by the device that created it (a UUID or
//! ULID), so pushing the same record twice is harmless. Times are Unix time
//! in milliseconds, from the device's clock.

use serde::{Deserialize, Serialize};

/// Most records a device may send in one push. Larger backlogs are sent in
/// several pushes.
pub const MAX_RECORDS_PER_PUSH: usize = 500;
/// Longest text accepted for one dictation, in bytes.
pub const MAX_TEXT_BYTES: usize = 200_000;

/// One dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictationRecord {
    /// Chosen by the device, unique across devices.
    pub id: String,
    /// When it was dictated.
    pub created_at: i64,
    /// When its text last changed (equal to `created_at` unless corrected).
    pub updated_at: i64,
    /// The text as pasted, or as corrected by the user afterwards.
    pub text: String,
    /// Language code such as `fr`, when known.
    #[serde(default)]
    pub language: Option<String>,
    /// Length of the recording, when known.
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// One replacement rule. A deleted rule stays as a tombstone so the deletion
/// reaches every device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleRecord {
    /// Chosen by the device that created the rule.
    pub id: String,
    /// Words to look for.
    pub pattern: String,
    /// Text that replaces them.
    pub replacement: String,
    /// Disabled rules sync but are not applied.
    pub enabled: bool,
    /// `manual` or `learned`.
    pub source: String,
    /// When the rule last changed. The newest change wins.
    pub updated_at: i64,
    /// True for a deleted rule (a tombstone).
    #[serde(default)]
    pub deleted: bool,
}

/// One word replacement the user made while correcting a dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutionRecord {
    /// Chosen by the device.
    pub id: String,
    /// What the transcription said.
    pub from: String,
    /// What the user changed it to.
    pub to: String,
    /// When the correction was made.
    pub created_at: i64,
}

/// One line of a meeting transcript, as synced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingLineRecord {
    /// Start, in milliseconds from the start of the meeting.
    pub start_ms: i64,
    /// End, in milliseconds from the start of the meeting.
    pub end_ms: i64,
    /// `you` or `others`.
    pub speaker: String,
    /// What was said.
    pub text: String,
}

/// A finished meeting with its whole transcript. Pushing it again with a
/// newer `updated_at` (a rename, a new summary) replaces it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingRecord {
    /// Chosen by the device that recorded it.
    pub id: String,
    /// The meeting's title.
    pub title: String,
    /// When it started, Unix ms.
    pub started_at: i64,
    /// Length of the recording.
    pub duration_ms: i64,
    /// When it last changed. The newest version wins.
    pub updated_at: i64,
    /// The summary as Markdown, if one was made.
    #[serde(default)]
    pub summary: Option<String>,
    /// The transcript, in order.
    pub lines: Vec<MeetingLineRecord>,
}

/// Most transcript lines one meeting may carry (about 20 hours of meeting).
pub const MAX_MEETING_LINES: usize = 5_000;

/// Body of `POST /api/sync/push`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    /// New or corrected dictations.
    #[serde(default)]
    pub dictations: Vec<DictationRecord>,
    /// Rules created, changed or deleted on the device.
    #[serde(default)]
    pub rules: Vec<RuleRecord>,
    /// Word replacements from the user's corrections.
    #[serde(default)]
    pub substitutions: Vec<SubstitutionRecord>,
    /// Finished meetings, new or changed.
    #[serde(default)]
    pub meetings: Vec<MeetingRecord>,
}

impl PushRequest {
    /// Total number of records in the push.
    pub fn len(&self) -> usize {
        self.dictations.len() + self.rules.len() + self.substitutions.len() + self.meetings.len()
    }

    /// True when there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Answer to a push: how many records changed something on the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    /// Dictations added or updated.
    pub dictations: usize,
    /// Rule changes accepted (older ones are ignored).
    pub rules: usize,
    /// Substitutions added.
    pub substitutions: usize,
    /// Meetings added or updated.
    #[serde(default)]
    pub meetings: usize,
}

/// Answer to `GET /api/sync/pull?since=<cursor>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullResponse {
    /// Rules that changed after the cursor the device sent, tombstones included.
    pub rules: Vec<RuleRecord>,
    /// Send this as `since` next time.
    pub cursor: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_fields_may_be_left_out() {
        let request: PushRequest = serde_json::from_str(
            r#"{"dictations":[{"id":"d1","created_at":1,"updated_at":1,"text":"bonjour"}],
                "rules":[{"id":"r1","pattern":"a","replacement":"b","enabled":true,"source":"manual","updated_at":2}]}"#,
        )
        .expect("valid push");
        assert_eq!(request.len(), 2);
        assert_eq!(request.dictations[0].language, None);
        assert!(!request.rules[0].deleted);
        assert!(request.substitutions.is_empty());
    }

    #[test]
    fn an_empty_body_is_an_empty_push() {
        let request: PushRequest = serde_json::from_str("{}").expect("valid");
        assert!(request.is_empty());
    }
}
