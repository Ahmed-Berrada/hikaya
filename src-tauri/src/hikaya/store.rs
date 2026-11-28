//! Storage for what Hikaya learns: replacement rules, the corrections the
//! user made, and the proposals they turned down.
//!
//! It is a separate SQLite file (`hikaya.db`) next to Handy's `history.db`,
//! so Handy's schema and migrations are never touched.

use anyhow::{Context, Result};
use hikaya_core::corrections::{self, Substitution};
use hikaya_core::rules::Rule;
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// A correction must be seen this many times before it is proposed as a rule.
pub const PROPOSAL_THRESHOLD: usize = 2;

const MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE rules (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        pattern TEXT NOT NULL,
        replacement TEXT NOT NULL,
        app TEXT,
        enabled INTEGER NOT NULL DEFAULT 1,
        source TEXT NOT NULL DEFAULT 'manual',
        created_at INTEGER NOT NULL
    );
    CREATE TABLE substitutions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        history_id INTEGER,
        from_text TEXT NOT NULL,
        to_text TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE dismissed_proposals (
        pattern TEXT NOT NULL,
        replacement TEXT NOT NULL,
        PRIMARY KEY (pattern, replacement)
    );",
    ),
    // Sync (roadmap v0.3): stable ids, change times and tombstones so rules
    // and corrections can be merged with the hub, plus the outgoing queue.
    M::up(
        "ALTER TABLE rules ADD COLUMN uid TEXT;
    ALTER TABLE rules ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE rules ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE rules ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;
    UPDATE rules SET uid = lower(hex(randomblob(16))), updated_at = created_at * 1000;
    CREATE UNIQUE INDEX rules_uid ON rules (uid);
    ALTER TABLE substitutions ADD COLUMN uid TEXT;
    ALTER TABLE substitutions ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;
    UPDATE substitutions SET uid = lower(hex(randomblob(16)));
    CREATE UNIQUE INDEX substitutions_uid ON substitutions (uid);
    CREATE TABLE sync_outbox (
        uid TEXT PRIMARY KEY,
        history_id INTEGER,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        text TEXT NOT NULL,
        language TEXT
    );
    CREATE TABLE sync_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    ),
    // Meetings (roadmap v0.4, ADR 0003).
    M::up(
        "CREATE TABLE meetings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        uid TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL DEFAULT 'recording',
        system_audio INTEGER NOT NULL DEFAULT 0,
        summary TEXT,
        summary_model TEXT,
        updated_at INTEGER NOT NULL,
        synced INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE meeting_lines (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        start_ms INTEGER NOT NULL,
        end_ms INTEGER NOT NULL,
        speaker TEXT NOT NULL,
        text TEXT NOT NULL
    );
    CREATE INDEX meeting_lines_meeting ON meeting_lines (meeting_id, start_ms);",
    ),
    // Usage counters for the stats panel. Kept apart from Handy's history,
    // which is pruned.
    M::up(
        "CREATE TABLE daily_stats (
        day TEXT PRIMARY KEY,
        dictations INTEGER NOT NULL DEFAULT 0,
        words INTEGER NOT NULL DEFAULT 0
    );",
    ),
    // Meetings made from phone recordings (ADR 0004): which recording, and
    // whether the hub has been told it is done.
    M::up(
        "ALTER TABLE meetings ADD COLUMN phone_recording TEXT;
    ALTER TABLE meetings ADD COLUMN phone_completed INTEGER NOT NULL DEFAULT 0;
    CREATE UNIQUE INDEX meetings_phone_recording ON meetings (phone_recording)
        WHERE phone_recording IS NOT NULL;",
    ),
    // Records the hub refused for good (too large, invalid, not ours): set
    // aside so they stop blocking everything queued behind them.
    M::up(
        "CREATE TABLE sync_refused (
        kind TEXT NOT NULL,
        uid TEXT NOT NULL,
        reason TEXT NOT NULL,
        at INTEGER NOT NULL,
        PRIMARY KEY (kind, uid)
    );",
    ),
    // The custom-words dictionary as last sent to or received from the hub.
    // Handy keeps the list in its settings; comparing the two finds what the
    // user added or removed. A removed word stays as a tombstone until sent.
    // `applied = 0` marks a change pulled from the hub that is not in the
    // settings yet, so it is never mistaken for a change the user made.
    M::up(
        "CREATE TABLE custom_words (
        word TEXT PRIMARY KEY,
        updated_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0,
        synced INTEGER NOT NULL DEFAULT 0,
        applied INTEGER NOT NULL DEFAULT 1
    );",
    ),
    // Hikaya's own preferences, kept out of Handy's settings so upstream
    // merges never touch them. A missing key means the default.
    M::up("CREATE TABLE prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL);"),
    // Snippets: a spoken cue that types a saved text. `synced`: 0 to send,
    // 1 sent, 2 refused by the hub. A deleted one stays until sent.
    M::up(
        "CREATE TABLE snippets (
        uid TEXT PRIMARY KEY,
        cue TEXT NOT NULL,
        text TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0,
        synced INTEGER NOT NULL DEFAULT 0
    );",
    ),
    // Telling speakers apart (ADR 0005): the voice of a line within its
    // track, and the names the user gave the voices of a meeting.
    M::up(
        "ALTER TABLE meeting_lines ADD COLUMN voice INTEGER;
    CREATE TABLE meeting_voices (
        meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        voice INTEGER NOT NULL,
        name TEXT NOT NULL,
        PRIMARY KEY (meeting_id, voice)
    );",
    ),
];

/// Hikaya's own preferences, as the UI reads and writes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Prefs {
    /// "New line", "scratch that" and their French forms act as commands.
    pub voice_commands: bool,
    /// The cleanup and per-app rules follow the app dictated into.
    pub adapt_to_app: bool,
    /// Beta: after a meeting, split the "Others" track by speaker.
    pub tell_speakers_apart: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            voice_commands: true,
            adapt_to_app: true,
            tell_speakers_apart: false,
        }
    }
}

/// A replacement rule as stored and as shown in the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StoredRule {
    pub id: i64,
    /// Words to look for (whole words, case-insensitive).
    pub pattern: String,
    /// Text that replaces a match.
    pub replacement: String,
    pub enabled: bool,
    /// `manual` when the user typed it, `learned` when accepted from a proposal.
    pub source: String,
    /// Unix time in seconds.
    pub created_at: i64,
    /// Unix time in milliseconds of the last change.
    pub updated_at: i64,
}

/// A rule suggested from the user's repeated corrections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RuleProposal {
    pub pattern: String,
    pub replacement: String,
    /// How many times the user made this correction.
    pub count: u32,
}

/// One word replacement found in a correction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LearnedSubstitution {
    pub from: String,
    pub to: String,
}

/// Current time in Unix milliseconds.
pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub struct LearningStore {
    db_path: PathBuf,
    /// Enabled rules, cached so a dictation never waits on the database.
    active: RwLock<Vec<Rule>>,
}

impl LearningStore {
    /// Opens the database at `db_path`, creating or migrating it as needed.
    pub fn open(db_path: &Path) -> Result<Self> {
        let mut conn =
            Connection::open(db_path).with_context(|| format!("opening {}", db_path.display()))?;
        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .context("migrating the learning database")?;
        let store = Self {
            db_path: db_path.to_path_buf(),
            active: RwLock::new(Vec::new()),
        };
        store.refresh_active()?;
        Ok(store)
    }

    /// A store with no database behind it: no rules, and every command fails
    /// with an error instead of the app refusing to start.
    pub fn unavailable() -> Self {
        Self {
            db_path: PathBuf::from("/nonexistent/hikaya-learning-unavailable/hikaya.db"),
            active: RwLock::new(Vec::new()),
        }
    }

    pub(super) fn conn(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    pub(super) fn refresh_active(&self) -> Result<()> {
        // Two devices can each create a rule for the same words; the most
        // recently changed one is the one applied.
        let mut seen = std::collections::HashSet::new();
        let mut enabled: Vec<StoredRule> = self
            .list_rules()?
            .into_iter()
            .filter(|rule| rule.enabled)
            .collect();
        enabled.sort_by_key(|rule| std::cmp::Reverse(rule.updated_at));
        let rules = enabled
            .into_iter()
            .filter(|rule| seen.insert(rule.pattern.to_lowercase()))
            .map(|rule| Rule::new(&rule.pattern, &rule.replacement))
            .collect();
        match self.active.write() {
            Ok(mut active) => *active = rules,
            Err(poisoned) => *poisoned.into_inner() = rules,
        }
        Ok(())
    }

    /// Hikaya's preferences; defaults for anything not set or unreadable.
    pub fn prefs(&self) -> Prefs {
        let read = || -> Result<Prefs> {
            let conn = self.conn()?;
            // Every preference is a switch; a missing one takes its default.
            let switch = |key: &str, default: bool| -> Result<bool> {
                let value: Option<String> = conn
                    .query_row("SELECT value FROM prefs WHERE key = ?1", [key], |row| {
                        row.get(0)
                    })
                    .optional()?;
                Ok(match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => default,
                })
            };
            let defaults = Prefs::default();
            Ok(Prefs {
                voice_commands: switch("voice_commands", defaults.voice_commands)?,
                adapt_to_app: switch("adapt_to_app", defaults.adapt_to_app)?,
                tell_speakers_apart: switch("tell_speakers_apart", defaults.tell_speakers_apart)?,
            })
        };
        read().unwrap_or_else(|e| {
            log::warn!("Could not read Hikaya's preferences, using the defaults: {e:#}");
            Prefs::default()
        })
    }

    /// Saves Hikaya's preferences.
    pub fn set_prefs(&self, prefs: Prefs) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for (key, on) in [
            ("voice_commands", prefs.voice_commands),
            ("adapt_to_app", prefs.adapt_to_app),
            ("tell_speakers_apart", prefs.tell_speakers_apart),
        ] {
            tx.execute(
                "INSERT INTO prefs (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = ?2",
                params![key, on.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The enabled rules, for the dictation pipeline.
    pub fn active_rules(&self) -> Vec<Rule> {
        match self.active.read() {
            Ok(active) => active.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Every rule except deleted ones, newest first.
    pub fn list_rules(&self) -> Result<Vec<StoredRule>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, pattern, replacement, enabled, source, created_at, updated_at
             FROM rules WHERE deleted = 0 ORDER BY id DESC",
        )?;
        let rules = statement
            .query_map([], |row| {
                Ok(StoredRule {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                    replacement: row.get(2)?,
                    enabled: row.get(3)?,
                    source: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rules)
    }

    /// Adds a rule. If a rule for the same words exists, its replacement is
    /// updated instead, so there is never more than one rule per pattern.
    pub fn add_rule(&self, pattern: &str, replacement: &str, source: &str) -> Result<StoredRule> {
        let pattern = pattern.trim();
        let replacement = replacement.trim();
        anyhow::ensure!(
            pattern.chars().any(char::is_alphanumeric),
            "a rule needs at least one word to look for"
        );
        anyhow::ensure!(!replacement.is_empty(), "a rule needs a replacement");

        let conn = self.conn()?;
        // A deleted rule for the same words is brought back rather than
        // duplicated, so its id (and its history on other devices) is kept.
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM rules WHERE lower(pattern) = lower(?1)
                 ORDER BY deleted, updated_at DESC LIMIT 1",
                params![pattern],
                |row| row.get(0),
            )
            .optional()?;
        let now = now_ms();
        let id = match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE rules SET replacement = ?1, enabled = 1, deleted = 0, updated_at = ?2, synced = 0
                     WHERE id = ?3",
                    params![replacement, now, id],
                )?;
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO rules (pattern, replacement, source, created_at, uid, updated_at)
                     VALUES (?1, ?2, ?3, ?4, lower(hex(randomblob(16))), ?5)",
                    params![pattern, replacement, source, now / 1000, now],
                )?;
                conn.last_insert_rowid()
            }
        };
        drop(conn);
        self.refresh_active()?;
        self.list_rules()?
            .into_iter()
            .find(|rule| rule.id == id)
            .context("the rule that was just saved is missing")
    }

    /// Turns a rule on or off.
    pub fn set_rule_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn()?.execute(
            "UPDATE rules SET enabled = ?1, updated_at = ?2, synced = 0 WHERE id = ?3",
            params![enabled, now_ms(), id],
        )?;
        self.refresh_active()
    }

    /// Deletes a rule. It is kept as a tombstone until the hub has seen the
    /// deletion, so the rule also disappears from the user's other devices.
    pub fn delete_rule(&self, id: i64) -> Result<()> {
        self.conn()?.execute(
            "UPDATE rules SET deleted = 1, updated_at = ?1, synced = 0 WHERE id = ?2",
            params![now_ms(), id],
        )?;
        self.refresh_active()
    }

    /// Records the word replacements between a dictation and the user's
    /// corrected version, and returns them.
    pub fn record_correction(
        &self,
        history_id: Option<i64>,
        before: &str,
        after: &str,
    ) -> Result<Vec<LearnedSubstitution>> {
        let found = corrections::extract_substitutions(before, after);
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().timestamp();
        for substitution in &found {
            tx.execute(
                "INSERT INTO substitutions (history_id, from_text, to_text, created_at, uid)
                 VALUES (?1, ?2, ?3, ?4, lower(hex(randomblob(16))))",
                params![history_id, substitution.from, substitution.to, now],
            )?;
        }
        tx.commit()?;
        Ok(found
            .into_iter()
            .map(|s| LearnedSubstitution {
                from: s.from,
                to: s.to,
            })
            .collect())
    }

    /// Rules worth suggesting: corrections seen at least
    /// [`PROPOSAL_THRESHOLD`] times that are not yet a rule and that the
    /// user has not dismissed.
    pub fn proposals(&self) -> Result<Vec<RuleProposal>> {
        let conn = self.conn()?;
        let history = conn
            .prepare("SELECT from_text, to_text FROM substitutions")?
            .query_map([], |row| {
                Ok(Substitution {
                    from: row.get(0)?,
                    to: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let dismissed = conn
            .prepare("SELECT pattern, replacement FROM dismissed_proposals")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(conn);

        // Disabled rules count too: the user already decided about those words.
        let existing: Vec<Rule> = self
            .list_rules()?
            .into_iter()
            .map(|rule| Rule::new(&rule.pattern, &rule.replacement))
            .collect();

        Ok(
            corrections::propose_rules(&history, &existing, PROPOSAL_THRESHOLD)
                .into_iter()
                .filter(|proposal| {
                    !dismissed.iter().any(|(pattern, replacement)| {
                        *pattern == proposal.rule.pattern
                            && *replacement == proposal.rule.replacement
                    })
                })
                .map(|proposal| RuleProposal {
                    pattern: proposal.rule.pattern,
                    replacement: proposal.rule.replacement,
                    count: u32::try_from(proposal.count).unwrap_or(u32::MAX),
                })
                .collect(),
        )
    }

    /// Turns a proposal into a rule.
    pub fn accept_proposal(&self, pattern: &str, replacement: &str) -> Result<StoredRule> {
        self.add_rule(pattern, replacement, "learned")
    }

    /// Hides a proposal for good.
    pub fn dismiss_proposal(&self, pattern: &str, replacement: &str) -> Result<()> {
        self.conn()?.execute(
            "INSERT OR IGNORE INTO dismissed_proposals (pattern, replacement) VALUES (?1, ?2)",
            params![pattern.to_lowercase(), replacement],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hikaya::stage::{apply_stage, StageInput};

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open store");
        (dir, store)
    }

    #[test]
    fn migrations_are_valid() {
        Migrations::new(MIGRATIONS.to_vec())
            .validate()
            .expect("valid migrations");
    }

    #[test]
    fn rules_survive_reopening_the_database() {
        let (dir, store) = store();
        store.add_rule("get hub", "GitHub", "manual").expect("add");
        drop(store);
        let reopened = LearningStore::open(&dir.path().join("hikaya.db")).expect("reopen");
        assert_eq!(
            reopened.active_rules(),
            vec![Rule::new("get hub", "GitHub")]
        );
    }

    #[test]
    fn adding_the_same_pattern_updates_the_rule() {
        let (_dir, store) = store();
        let first = store.add_rule("clod", "cloud", "manual").expect("add");
        let second = store.add_rule("Clod", "Claude", "manual").expect("update");
        assert_eq!(first.id, second.id);
        assert_eq!(second.replacement, "Claude");
        assert_eq!(store.list_rules().expect("list").len(), 1);
    }

    #[test]
    fn rejects_rules_with_no_word_or_no_replacement() {
        let (_dir, store) = store();
        assert!(store.add_rule("...", "x", "manual").is_err());
        assert!(store.add_rule("word", "  ", "manual").is_err());
    }

    #[test]
    fn disabled_and_deleted_rules_stop_applying() {
        let (_dir, store) = store();
        let rule = store.add_rule("clod", "Claude", "manual").expect("add");
        store.set_rule_enabled(rule.id, false).expect("disable");
        assert!(store.active_rules().is_empty());
        store.set_rule_enabled(rule.id, true).expect("enable");
        assert_eq!(store.active_rules().len(), 1);
        store.delete_rule(rule.id).expect("delete");
        assert!(store.active_rules().is_empty());
        assert!(store.list_rules().expect("list").is_empty());
    }

    #[test]
    fn a_correction_made_twice_becomes_a_proposal_then_a_rule() {
        let (_dir, store) = store();
        let found = store
            .record_correction(
                Some(1),
                "merci a med pour la demo",
                "merci Ahmed pour la demo",
            )
            .expect("record");
        assert_eq!(
            found,
            vec![LearnedSubstitution {
                from: "a med".to_string(),
                to: "Ahmed".to_string()
            }]
        );
        assert!(store.proposals().expect("proposals").is_empty());

        store
            .record_correction(Some(2), "ask a med", "ask Ahmed")
            .expect("record");
        let proposals = store.proposals().expect("proposals");
        assert_eq!(
            proposals,
            vec![RuleProposal {
                pattern: "a med".to_string(),
                replacement: "Ahmed".to_string(),
                count: 2
            }]
        );

        let rule = store.accept_proposal("a med", "Ahmed").expect("accept");
        assert_eq!(rule.source, "learned");
        assert!(store.proposals().expect("proposals").is_empty());

        // The next dictation comes out right.
        let languages = vec!["en".to_string(), "fr".to_string()];
        let rules = store.active_rules();
        assert_eq!(
            apply_stage(
                "Euh, demande à a med.",
                &StageInput {
                    languages: &languages,
                    remove_fillers: true,
                    extra_fillers: &[],
                    rules: &rules,
                    voice_commands: true,
                    app: None,
                    command: false,
                    snippets: &[],
                }
            ),
            "Demande à Ahmed."
        );
    }

    #[test]
    fn a_dismissed_proposal_does_not_come_back() {
        let (_dir, store) = store();
        for _ in 0..3 {
            store
                .record_correction(None, "use clod", "use Claude")
                .expect("record");
        }
        assert_eq!(store.proposals().expect("proposals").len(), 1);
        store.dismiss_proposal("clod", "Claude").expect("dismiss");
        assert!(store.proposals().expect("proposals").is_empty());
        store
            .record_correction(None, "use clod", "use Claude")
            .expect("record");
        assert!(store.proposals().expect("proposals").is_empty());
    }

    #[test]
    fn an_unavailable_store_has_no_rules_and_returns_errors() {
        let store = LearningStore::unavailable();
        assert!(store.active_rules().is_empty());
        assert!(store.list_rules().is_err());
        assert!(store.add_rule("a", "b", "manual").is_err());
    }

    #[test]
    fn an_edit_with_no_word_replacement_records_nothing() {
        let (_dir, store) = store();
        let found = store
            .record_correction(None, "we ship friday", "we ship on friday.")
            .expect("record");
        assert!(found.is_empty());
        let found = store
            .record_correction(None, "same text", "same text")
            .expect("record");
        assert!(found.is_empty());
    }

    #[test]
    fn prefs_default_to_voice_commands_on_and_persist() {
        let (_dir, store) = store();
        assert!(store.prefs().voice_commands);
        // A beta stays off until the user turns it on.
        assert!(!store.prefs().tell_speakers_apart);
        store
            .set_prefs(Prefs {
                voice_commands: false,
                adapt_to_app: true,
                tell_speakers_apart: true,
            })
            .expect("save");
        assert!(!store.prefs().voice_commands);
        assert!(store.prefs().adapt_to_app);
        assert!(store.prefs().tell_speakers_apart);
        store.set_prefs(Prefs::default()).expect("save");
        assert!(store.prefs().voice_commands);
    }
}
