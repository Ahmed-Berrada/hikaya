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

/// One word of the user's custom-words dictionary. The word is its own id,
/// so two devices adding the same word make one record. A removed word stays
/// as a tombstone so the removal reaches every device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordRecord {
    /// The word as the user typed it, trimmed.
    pub word: String,
    /// When it was added or removed. The newest change wins.
    pub updated_at: i64,
    /// True for a removed word (a tombstone).
    #[serde(default)]
    pub deleted: bool,
}

/// One snippet: a spoken cue and the text it types. Shared by every device
/// like rules: the newest change wins, and a deleted one stays as a
/// tombstone so the deletion reaches every device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetRecord {
    /// Chosen by the device that created the snippet.
    pub id: String,
    /// What the user says.
    pub cue: String,
    /// What gets typed, line breaks included.
    pub text: String,
    /// When it last changed. The newest change wins.
    pub updated_at: i64,
    /// True for a deleted snippet (a tombstone).
    #[serde(default)]
    pub deleted: bool,
}

/// Longest custom word accepted, in bytes.
pub const MAX_WORD_BYTES: usize = 100;

/// The form a custom word is stored and compared in: trimmed. `None` when
/// nothing is left, the word is too long or contains a control character.
pub fn clean_word(word: &str) -> Option<&str> {
    let word = word.trim();
    let ok =
        !word.is_empty() && word.len() <= MAX_WORD_BYTES && !word.chars().any(char::is_control);
    ok.then_some(word)
}

/// Applies word changes from the hub to a custom-words list: an added word
/// is appended if missing, a removed one is taken out. Words are compared
/// trimmed and case-sensitively, like the hub's ids. True if the list changed.
pub fn apply_word_changes(list: &mut Vec<String>, changes: &[WordRecord]) -> bool {
    let before = list.len();
    let mut added = false;
    for change in changes {
        if change.deleted {
            list.retain(|w| clean_word(w) != Some(change.word.as_str()));
        } else if !list
            .iter()
            .any(|w| clean_word(w) == Some(change.word.as_str()))
        {
            list.push(change.word.clone());
            added = true;
        }
    }
    added || list.len() != before
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
    /// Custom words added or removed.
    #[serde(default)]
    pub words: Vec<WordRecord>,
    /// Snippets created, changed or deleted.
    #[serde(default)]
    pub snippets: Vec<SnippetRecord>,
}

impl PushRequest {
    /// Total number of records in the push.
    pub fn len(&self) -> usize {
        self.dictations.len()
            + self.rules.len()
            + self.substitutions.len()
            + self.meetings.len()
            + self.words.len()
            + self.snippets.len()
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
    /// Custom-word changes accepted (older ones are ignored). Always present
    /// from a hub that knows custom words; `None` means an older hub that
    /// ignored them, so the device must keep them queued.
    #[serde(default)]
    pub words: Option<usize>,
    /// Snippet changes accepted; `None` from a hub that predates snippets.
    #[serde(default)]
    pub snippets: Option<usize>,
}

/// Answer to `GET /api/sync/pull?since=<cursor>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullResponse {
    /// Rules that changed after the cursor the device sent, tombstones included.
    pub rules: Vec<RuleRecord>,
    /// Custom words that changed after the cursor, tombstones included.
    #[serde(default)]
    pub words: Vec<WordRecord>,
    /// Snippets that changed after the cursor, tombstones included.
    #[serde(default)]
    pub snippets: Vec<SnippetRecord>,
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
    fn a_pull_from_an_older_hub_has_no_words() {
        let pulled: PullResponse =
            serde_json::from_str(r#"{"rules":[],"cursor":3}"#).expect("valid pull");
        assert!(pulled.words.is_empty());
    }

    #[test]
    fn words_are_trimmed_and_checked() {
        assert_eq!(clean_word("  GitHub "), Some("GitHub"));
        assert_eq!(clean_word("   "), None);
        assert_eq!(clean_word("a\tb"), None);
        assert_eq!(clean_word(&"x".repeat(MAX_WORD_BYTES + 1)), None);
        assert_eq!(clean_word("Kubernetes"), Some("Kubernetes"));
    }

    fn change(word: &str, deleted: bool) -> WordRecord {
        WordRecord {
            word: word.into(),
            updated_at: 1,
            deleted,
        }
    }

    #[test]
    fn word_changes_add_once_and_remove_every_spelling_with_spaces() {
        let mut list = vec!["Hikaya".to_string(), " Ollama ".to_string()];
        assert!(apply_word_changes(
            &mut list,
            &[
                change("GitHub", false),
                change("Hikaya", false),
                change("Ollama", true)
            ]
        ));
        assert_eq!(list, ["Hikaya", "GitHub"]);
        assert!(!apply_word_changes(
            &mut list,
            &[change("GitHub", false), change("gone", true)]
        ));
        // Case matters: "github" is another word.
        assert!(apply_word_changes(&mut list, &[change("github", false)]));
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn an_empty_body_is_an_empty_push() {
        let request: PushRequest = serde_json::from_str("{}").expect("valid");
        assert!(request.is_empty());
    }
}
