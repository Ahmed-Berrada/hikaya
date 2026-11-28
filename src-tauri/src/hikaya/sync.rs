//! Sends this device's dictations, rules, corrections, meetings and custom
//! words to the hub, and brings rule and word changes made on other devices
//! back.
//!
//! Dictations are copied into a queue (`sync_outbox`) the moment they are
//! saved, because Handy prunes its own history. A queued dictation leaves the
//! queue only once the hub has accepted it. Nothing is queued while no hub is
//! connected; connecting queues the history that exists at that moment.
//!
//! The hub answers `423 Locked` after it restarts, until its passphrase is
//! entered. That is not an error here: the queue simply waits.

use super::store::{now_ms, LearningStore};
use anyhow::{Context, Result};
use hikaya_core::sync::{
    clean_word, DictationRecord, MeetingLineRecord, MeetingRecord, PullResponse, PushRequest,
    PushResponse, RuleRecord, SnippetRecord, SubstitutionRecord, WordRecord,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::future::Future;
use std::time::Duration;

/// Dictations sent per push. Rules, corrections, words and snippets ride along.
const DICTATIONS_PER_PUSH: usize = 150;
/// Rules, corrections and custom words each sent per push, at most.
const OTHERS_PER_PUSH: usize = 100;
/// Snippets sent per push, at most: each can hold a few kilobytes of text.
const SNIPPETS_PER_PUSH: usize = 50;
// A full batch must fit in one push, or the hub refuses it as too large.
const _: () = assert!(
    DICTATIONS_PER_PUSH + 3 * OTHERS_PER_PUSH + SNIPPETS_PER_PUSH
        <= hikaya_core::sync::MAX_RECORDS_PER_PUSH
);
/// Meetings sent per push: each carries its whole transcript.
const MEETINGS_PER_PUSH: usize = 1;
/// Meetings in these states are finished and can be sent.
const FINISHED: &str = "('done', 'interrupted')";
/// Safety stop for one sync run; the rest goes in the next run.
const MAX_PUSHES_PER_RUN: usize = 20;
/// How long a request may take before the hub counts as unreachable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// A push that carries a whole meeting transcript may take longer on a slow
/// link than a batch of dictations.
const MEETING_TIMEOUT: Duration = Duration::from_secs(120);

/// What went wrong talking to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubError {
    /// No answer: wrong address, no network, or the hub is down.
    Offline(String),
    /// The hub restarted and waits for its passphrase.
    Locked,
    /// The device token is unknown or was revoked.
    Unauthorized,
    /// The hub refused the request, with its reason.
    Rejected(String),
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline(why) => write!(f, "the hub can't be reached: {why}"),
            Self::Locked => write!(f, "the hub is locked"),
            Self::Unauthorized => write!(f, "this device is not allowed on the hub"),
            Self::Rejected(why) => write!(f, "the hub refused: {why}"),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for HubError {}

/// How this device reaches the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubConfig {
    pub url: String,
    pub token: String,
    pub device_name: String,
    /// Prefix of this device's dictation ids, unique per installation.
    pub device_uid: String,
}

/// The hub operations sync needs. A trait so tests can stand in for the hub.
pub trait HubApi {
    fn push(&self, request: &PushRequest) -> impl Future<Output = Result<PushResponse, HubError>>;
    fn pull(&self, since: i64) -> impl Future<Output = Result<PullResponse, HubError>>;
}

/// What one sync run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub sent_dictations: usize,
    pub sent_rules: usize,
    pub received_rules: usize,
    /// Custom-word changes from other devices, now waiting in the table to
    /// be applied to the settings (see `pending_word_changes`).
    pub received_words: usize,
    /// The hub ignored custom words (it predates them); they stay queued.
    pub words_unsupported: bool,
    /// Snippet changes from other devices applied here.
    pub received_snippets: usize,
    /// The hub ignored snippets (it predates them); they stay queued.
    pub snippets_unsupported: bool,
    /// Records the hub refused for good in this run; they are set aside.
    pub refused: usize,
    /// The run stopped at its push limit with records still queued.
    pub more: bool,
}

// ---------------------------------------------------------------- storage --

fn setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM sync_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

impl LearningStore {
    /// The hub this device is connected to, if any.
    pub fn hub_config(&self) -> Result<Option<HubConfig>> {
        let conn = self.conn()?;
        let (Some(url), Some(token)) = (setting(&conn, "hub_url")?, setting(&conn, "hub_token")?)
        else {
            return Ok(None);
        };
        Ok(Some(HubConfig {
            url,
            token,
            device_name: setting(&conn, "device_name")?.unwrap_or_default(),
            device_uid: self.device_uid_with(&conn)?,
        }))
    }

    fn device_uid_with(&self, conn: &Connection) -> Result<String> {
        if let Some(uid) = setting(conn, "device_uid")? {
            return Ok(uid);
        }
        let uid: String =
            conn.query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        set_setting(conn, "device_uid", &uid)?;
        Ok(uid)
    }

    /// Saves the connection to a hub.
    pub fn save_hub_config(&self, url: &str, token: &str, device_name: &str) -> Result<HubConfig> {
        let conn = self.conn()?;
        if setting(&conn, "hub_url")?.as_deref() != Some(url) {
            // A different hub has none of this device's rules, corrections
            // or meetings: send them all again, and retry what the old hub
            // refused.
            conn.execute_batch(
                "UPDATE rules SET synced = 0;
                 UPDATE substitutions SET synced = 0;
                 UPDATE custom_words SET synced = 0;
                 UPDATE snippets SET synced = 0;
                 UPDATE meetings SET synced = 0, phone_completed = 1
                   WHERE status IN ('done', 'interrupted');
                 DELETE FROM sync_refused;",
            )?;
        }
        set_setting(&conn, "hub_url", url)?;
        set_setting(&conn, "hub_token", token)?;
        set_setting(&conn, "device_name", device_name)?;
        // A different hub has its own feed: start pulling from the beginning.
        conn.execute("DELETE FROM sync_settings WHERE key = 'pull_cursor'", [])?;
        drop(conn);
        self.hub_config()?
            .context("the hub settings were not saved")
    }

    /// Forgets the hub and empties the queue. Rules stay on this device.
    pub fn clear_hub_config(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "DELETE FROM sync_settings WHERE key IN ('hub_url', 'hub_token', 'pull_cursor', 'last_sync_at');
             DELETE FROM sync_outbox;",
        )?;
        Ok(())
    }

    /// Queues a dictation (or its corrected text) for the hub. Does nothing
    /// while no hub is connected.
    pub fn queue_dictation(
        &self,
        history_id: i64,
        created_at_ms: i64,
        text: &str,
        language: Option<&str>,
    ) -> Result<bool> {
        let Some(config) = self.hub_config()? else {
            return Ok(false);
        };
        if text.trim().is_empty() {
            return Ok(false);
        }
        let uid = format!("{}-{history_id}", config.device_uid);
        // updated_at can never be before created_at, even if the clock moved.
        let updated_at = now_ms().max(created_at_ms);
        self.conn()?.execute(
            "INSERT INTO sync_outbox (uid, history_id, created_at, updated_at, text, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (uid) DO UPDATE SET text = ?5, updated_at = ?4, language = ?6",
            params![uid, history_id, created_at_ms, updated_at, text, language],
        )?;
        Ok(true)
    }

    /// Dictations, rules and corrections waiting to be sent.
    pub fn pending_count(&self) -> Result<u32> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT (SELECT count(*) FROM sync_outbox)
                  + (SELECT count(*) FROM rules WHERE synced = 0)
                  + (SELECT count(*) FROM substitutions WHERE synced = 0)
                  + (SELECT count(*) FROM meetings WHERE synced = 0 AND status IN ('done', 'interrupted'))",
            [],
            |row| row.get(0),
        )?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Records the hub refused for good, set aside.
    /// Compares the custom words in the settings with those last synced, and
    /// queues what the user added or removed since. Run before each sync.
    pub fn note_custom_words(&self, words: &[String]) -> Result<()> {
        let current: std::collections::HashSet<&str> =
            words.iter().filter_map(|w| clean_word(w)).collect();
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        // Changes pulled from the hub but not yet in the settings are not
        // the user's doing: leave them for `mark_words_applied`.
        let known = tx
            .prepare("SELECT word, deleted, updated_at FROM custom_words WHERE applied = 1")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pending = tx
            .prepare("SELECT word FROM custom_words WHERE applied = 0")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        let now = now_ms();
        // A change made here must beat the version this device last saw,
        // even when this computer's clock is behind the one that made it.
        let stamp = |seen: i64| now.max(seen + 1);
        for (word, deleted, updated_at) in &known {
            if !deleted && !current.contains(word.as_str()) {
                tx.execute(
                    "UPDATE custom_words SET deleted = 1, updated_at = ?2, synced = 0 WHERE word = ?1",
                    params![word, stamp(*updated_at)],
                )?;
            }
        }
        for word in current {
            if pending.contains(word) {
                continue;
            }
            match known.iter().find(|(w, ..)| w == word) {
                Some((_, false, _)) => {}
                Some((_, true, updated_at)) => {
                    tx.execute(
                        "UPDATE custom_words SET deleted = 0, updated_at = ?2, synced = 0 WHERE word = ?1",
                        params![word, stamp(*updated_at)],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO custom_words (word, updated_at, deleted, synced) VALUES (?1, ?2, 0, 0)",
                        params![word, now],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn refused_count(&self) -> Result<u32> {
        let count: i64 =
            self.conn()?
                .query_row("SELECT count(*) FROM sync_refused", [], |row| row.get(0))?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Sets aside a record the hub refused, so it stops blocking the queue.
    /// An edited rule or meeting is sent again; a dictation leaves the queue
    /// (it stays in History on this computer).
    fn set_aside(&self, batch: &PushRequest, reason: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let now = now_ms();
        let note = |kind: &str, uid: &str| {
            tx.execute(
                "INSERT INTO sync_refused (kind, uid, reason, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (kind, uid) DO UPDATE SET reason = ?3, at = ?4",
                params![kind, uid, reason, now],
            )
            .map(|_| ())
        };
        for d in &batch.dictations {
            note("dictation", &d.id)?;
        }
        for r in &batch.rules {
            note("rule", &r.id)?;
        }
        for s in &batch.substitutions {
            note("correction", &s.id)?;
        }
        for m in &batch.meetings {
            note("meeting", &m.id)?;
        }
        for w in &batch.words {
            note("word", &w.word)?;
        }
        for s in &batch.snippets {
            note("snippet", &s.id)?;
        }
        for d in &batch.dictations {
            tx.execute("DELETE FROM sync_outbox WHERE uid = ?1", params![d.id])?;
        }
        for r in &batch.rules {
            tx.execute(
                "UPDATE rules SET synced = 2 WHERE uid = ?1 AND updated_at = ?2",
                params![r.id, r.updated_at],
            )?;
        }
        for s in &batch.substitutions {
            tx.execute(
                "UPDATE substitutions SET synced = 2 WHERE uid = ?1",
                params![s.id],
            )?;
        }
        for m in &batch.meetings {
            tx.execute(
                "UPDATE meetings SET synced = 2 WHERE uid = ?1 AND updated_at = ?2",
                params![m.id, m.updated_at],
            )?;
        }
        for w in &batch.words {
            tx.execute(
                "UPDATE custom_words SET synced = 2 WHERE word = ?1 AND updated_at = ?2",
                params![w.word, w.updated_at],
            )?;
        }
        for s in &batch.snippets {
            tx.execute(
                "UPDATE snippets SET synced = 2 WHERE uid = ?1 AND updated_at = ?2",
                params![s.id, s.updated_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Unix ms of the last successful sync.
    pub fn last_sync_at(&self) -> Result<Option<i64>> {
        Ok(setting(&self.conn()?, "last_sync_at")?.and_then(|v| v.parse().ok()))
    }

    fn next_batch(&self, with_words: bool, with_snippets: bool) -> Result<PushRequest> {
        let conn = self.conn()?;
        let dictations = conn
            .prepare(
                "SELECT uid, created_at, updated_at, text, language FROM sync_outbox
                 ORDER BY created_at LIMIT ?1",
            )?
            .query_map(params![DICTATIONS_PER_PUSH as i64], |row| {
                Ok(DictationRecord {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    updated_at: row.get(2)?,
                    text: row.get(3)?,
                    language: row.get(4)?,
                    duration_ms: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let rules = conn
            .prepare(
                "SELECT uid, pattern, replacement, enabled, source, updated_at, deleted
                 FROM rules WHERE synced = 0 LIMIT ?1",
            )?
            .query_map(params![OTHERS_PER_PUSH as i64], |row| {
                Ok(RuleRecord {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                    replacement: row.get(2)?,
                    enabled: row.get(3)?,
                    source: row.get(4)?,
                    updated_at: row.get(5)?,
                    deleted: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let substitutions = conn
            .prepare(
                "SELECT uid, from_text, to_text, created_at FROM substitutions
                 WHERE synced = 0 LIMIT ?1",
            )?
            .query_map(params![OTHERS_PER_PUSH as i64], |row| {
                Ok(SubstitutionRecord {
                    id: row.get(0)?,
                    from: row.get(1)?,
                    to: row.get(2)?,
                    // Stored in seconds; the wire uses milliseconds.
                    created_at: row.get::<_, i64>(3)? * 1000,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let words = conn
            .prepare(
                "SELECT word, updated_at, deleted FROM custom_words
                 WHERE synced = 0 AND ?1 LIMIT ?2",
            )?
            .query_map(params![with_words, OTHERS_PER_PUSH as i64], |row| {
                Ok(WordRecord {
                    word: row.get(0)?,
                    updated_at: row.get(1)?,
                    deleted: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let snippets = conn
            .prepare(
                "SELECT uid, cue, text, updated_at, deleted FROM snippets
                 WHERE synced = 0 AND ?1 LIMIT ?2",
            )?
            .query_map(params![with_snippets, SNIPPETS_PER_PUSH as i64], |row| {
                Ok(SnippetRecord {
                    id: row.get(0)?,
                    cue: row.get(1)?,
                    text: row.get(2)?,
                    updated_at: row.get(3)?,
                    deleted: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(PushRequest {
            dictations,
            rules,
            substitutions,
            meetings: Vec::new(),
            words,
            snippets,
        })
    }

    /// The next finished meeting to send, in a push of its own.
    fn next_meetings(&self) -> Result<Vec<MeetingRecord>> {
        let conn = self.conn()?;
        let finished = conn
            .prepare(&format!(
                "SELECT id, uid, title, started_at, duration_ms, updated_at, summary FROM meetings
                 WHERE synced = 0 AND status IN {FINISHED} ORDER BY started_at LIMIT ?1"
            ))?
            .query_map(params![MEETINGS_PER_PUSH as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    MeetingRecord {
                        id: row.get(1)?,
                        title: row.get(2)?,
                        started_at: row.get(3)?,
                        duration_ms: row.get(4)?,
                        updated_at: row.get(5)?,
                        summary: row.get(6)?,
                        lines: Vec::new(),
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(conn);
        let mut meetings = Vec::with_capacity(finished.len());
        for (id, mut meeting) in finished {
            meeting.lines = self
                .meeting_lines(id)?
                .into_iter()
                .map(|l| MeetingLineRecord {
                    start_ms: l.start_ms,
                    end_ms: l.end_ms,
                    speaker: l.speaker.as_str().into(),
                    text: l.text,
                    voice: l.voice,
                })
                .collect();
            meetings.push(meeting);
        }
        Ok(meetings)
    }

    /// Marks what the hub accepted. A record changed while the push was in
    /// flight keeps its newer state and is sent again next time.
    fn mark_sent(&self, batch: &PushRequest) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for d in &batch.dictations {
            tx.execute(
                "DELETE FROM sync_outbox WHERE uid = ?1 AND updated_at = ?2",
                params![d.id, d.updated_at],
            )?;
        }
        for r in &batch.rules {
            tx.execute(
                "UPDATE rules SET synced = 1 WHERE uid = ?1 AND updated_at = ?2",
                params![r.id, r.updated_at],
            )?;
        }
        for m in &batch.meetings {
            tx.execute(
                "UPDATE meetings SET synced = 1 WHERE uid = ?1 AND updated_at = ?2",
                params![m.id, m.updated_at],
            )?;
        }
        for x in &batch.snippets {
            tx.execute(
                "UPDATE snippets SET synced = 1 WHERE uid = ?1 AND updated_at = ?2",
                params![x.id, x.updated_at],
            )?;
            tx.execute(
                "DELETE FROM sync_refused WHERE kind = 'snippet' AND uid = ?1",
                params![x.id],
            )?;
        }
        // Tombstones the hub has seen are no longer needed here.
        tx.execute("DELETE FROM snippets WHERE deleted = 1 AND synced = 1", [])?;
        for w in &batch.words {
            tx.execute(
                "UPDATE custom_words SET synced = 1 WHERE word = ?1 AND updated_at = ?2",
                params![w.word, w.updated_at],
            )?;
            tx.execute(
                "DELETE FROM sync_refused WHERE kind = 'word' AND uid = ?1",
                params![w.word],
            )?;
        }
        for uid in batch
            .rules
            .iter()
            .map(|r| &r.id)
            .chain(batch.meetings.iter().map(|m| &m.id))
            .chain(batch.substitutions.iter().map(|s| &s.id))
        {
            tx.execute("DELETE FROM sync_refused WHERE uid = ?1", params![uid])?;
        }
        // Tombstones the hub has seen are no longer needed here.
        tx.execute("DELETE FROM rules WHERE deleted = 1 AND synced = 1", [])?;
        tx.execute(
            "DELETE FROM custom_words WHERE deleted = 1 AND synced = 1 AND applied = 1",
            [],
        )?;
        for s in &batch.substitutions {
            tx.execute(
                "UPDATE substitutions SET synced = 1 WHERE uid = ?1",
                params![s.id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Applies rule changes from the hub. The newest change wins; a local
    /// change that has not been sent yet and is newer is kept.
    fn merge_rules(&self, rules: &[RuleRecord]) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut applied = 0;
        for rule in rules {
            let local: Option<i64> = tx
                .query_row(
                    "SELECT updated_at FROM rules WHERE uid = ?1",
                    params![rule.id],
                    |row| row.get(0),
                )
                .optional()?;
            match local {
                Some(updated_at) if updated_at >= rule.updated_at => continue,
                Some(_) => {
                    tx.execute(
                        "UPDATE rules SET pattern = ?2, replacement = ?3, enabled = ?4, source = ?5,
                           updated_at = ?6, deleted = ?7, synced = 1 WHERE uid = ?1",
                        params![
                            rule.id,
                            rule.pattern,
                            rule.replacement,
                            rule.enabled,
                            rule.source,
                            rule.updated_at,
                            rule.deleted
                        ],
                    )?;
                }
                None if rule.deleted => continue,
                None => {
                    tx.execute(
                        "INSERT INTO rules (uid, pattern, replacement, enabled, source, created_at, updated_at, synced)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                        params![
                            rule.id,
                            rule.pattern,
                            rule.replacement,
                            rule.enabled,
                            rule.source,
                            rule.updated_at / 1000,
                            rule.updated_at
                        ],
                    )?;
                }
            }
            applied += 1;
        }
        tx.execute("DELETE FROM rules WHERE deleted = 1 AND synced = 1", [])?;
        tx.commit()?;
        drop(conn);
        self.refresh_active()?;
        Ok(applied)
    }

    /// Applies custom-word changes from the hub to the table (the newest
    /// change wins; an unsent newer local change is kept) and flags them for
    /// the settings. Returns how many won.
    fn merge_words(&self, words: &[WordRecord]) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut applied = 0;
        for w in words {
            let local: Option<i64> = tx
                .query_row(
                    "SELECT updated_at FROM custom_words WHERE word = ?1",
                    params![w.word],
                    |row| row.get(0),
                )
                .optional()?;
            match local {
                Some(updated_at) if updated_at >= w.updated_at => continue,
                // Never seen here and already removed elsewhere: nothing to do.
                None if w.deleted => continue,
                _ => {}
            }
            tx.execute(
                "INSERT INTO custom_words (word, updated_at, deleted, synced, applied)
                 VALUES (?1, ?2, ?3, 1, 0)
                 ON CONFLICT (word) DO UPDATE SET updated_at = ?2, deleted = ?3, synced = 1, applied = 0",
                params![w.word, w.updated_at, w.deleted],
            )?;
            applied += 1;
        }
        tx.commit()?;
        Ok(applied)
    }

    /// Word changes pulled from the hub that the settings do not have yet.
    /// They survive a crash or a failed run, and are applied on the next.
    pub fn pending_word_changes(&self) -> Result<Vec<WordRecord>> {
        Ok(self
            .conn()?
            .prepare("SELECT word, updated_at, deleted FROM custom_words WHERE applied = 0")?
            .query_map([], |row| {
                Ok(WordRecord {
                    word: row.get(0)?,
                    updated_at: row.get(1)?,
                    deleted: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Records that these pulled changes are now in the settings. A change
    /// replaced by a newer pull in the meantime stays pending.
    pub fn mark_words_applied(&self, changes: &[WordRecord]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for w in changes {
            tx.execute(
                "UPDATE custom_words SET applied = 1 WHERE word = ?1 AND updated_at = ?2",
                params![w.word, w.updated_at],
            )?;
        }
        tx.execute(
            "DELETE FROM custom_words WHERE deleted = 1 AND synced = 1 AND applied = 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Applies snippet changes from the hub. The newest change wins; a
    /// local change that has not been sent yet and is newer is kept.
    fn merge_snippets(&self, snippets: &[SnippetRecord]) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut applied = 0;
        for x in snippets {
            let local: Option<i64> = tx
                .query_row(
                    "SELECT updated_at FROM snippets WHERE uid = ?1",
                    params![x.id],
                    |row| row.get(0),
                )
                .optional()?;
            match local {
                Some(updated_at) if updated_at >= x.updated_at => continue,
                None if x.deleted => continue,
                _ => {}
            }
            tx.execute(
                "INSERT INTO snippets (uid, cue, text, updated_at, deleted, synced)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)
                 ON CONFLICT (uid) DO UPDATE SET cue = ?2, text = ?3, updated_at = ?4,
                   deleted = ?5, synced = 1",
                params![x.id, x.cue, x.text, x.updated_at, x.deleted],
            )?;
            applied += 1;
        }
        // Two devices may each have made the same cue: keep the newest, and
        // delete the others everywhere, so one cue types one text.
        let live = tx
            .prepare("SELECT uid, cue, updated_at FROM snippets WHERE deleted = 0")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut newest: std::collections::HashMap<String, (i64, String)> =
            std::collections::HashMap::new();
        for (uid, cue, updated_at) in &live {
            let key = hikaya_core::snippets::normalize_cue(cue);
            let entry = newest.entry(key).or_insert((*updated_at, uid.clone()));
            if (*updated_at, uid) > (entry.0, &entry.1) {
                *entry = (*updated_at, uid.clone());
            }
        }
        for (uid, cue, _) in &live {
            let key = hikaya_core::snippets::normalize_cue(cue);
            if newest.get(&key).is_some_and(|(_, keep)| keep != uid) {
                tx.execute(
                    "UPDATE snippets SET deleted = 1, updated_at = max(?1, updated_at + 1), synced = 0
                     WHERE uid = ?2",
                    params![now_ms(), uid],
                )?;
            }
        }
        tx.execute("DELETE FROM snippets WHERE deleted = 1 AND synced = 1", [])?;
        tx.commit()?;
        Ok(applied)
    }

    fn pull_cursor(&self) -> Result<i64> {
        let stored = setting(&self.conn()?, "pull_cursor")?;
        Ok(match stored.as_deref().map(str::parse::<i64>) {
            Some(Ok(cursor)) => cursor,
            Some(Err(_)) => {
                log::warn!("The saved pull cursor is unreadable; pulling all rules again");
                0
            }
            None => 0,
        })
    }
}

/// One full exchange with the hub: push everything queued, then pull.
pub async fn sync_once(store: &LearningStore, hub: &impl HubApi) -> Result<SyncReport, HubError> {
    let other = |e: anyhow::Error| HubError::Other(format!("{e:#}"));
    let mut report = SyncReport::default();
    let mut pushes = 0;
    // Dictations, rules and corrections first, then meetings one at a time:
    // a long meeting never holds up a dictation.
    loop {
        if pushes >= MAX_PUSHES_PER_RUN {
            report.more = true;
            break;
        }
        let batch = store
            .next_batch(!report.words_unsupported, !report.snippets_unsupported)
            .map_err(other)?;
        if batch.is_empty() {
            break;
        }
        let full = batch.dictations.len() >= DICTATIONS_PER_PUSH
            || batch.rules.len() >= OTHERS_PER_PUSH
            || batch.substitutions.len() >= OTHERS_PER_PUSH
            || batch.words.len() >= OTHERS_PER_PUSH
            || batch.snippets.len() >= SNIPPETS_PER_PUSH;
        pushes += push_or_set_aside(store, hub, batch, &mut report).await?;
        if !full {
            break;
        }
    }
    loop {
        if pushes >= MAX_PUSHES_PER_RUN {
            report.more = true;
            break;
        }
        let meetings = store.next_meetings().map_err(other)?;
        if meetings.is_empty() {
            break;
        }
        let batch = PushRequest {
            meetings,
            ..Default::default()
        };
        pushes += push_or_set_aside(store, hub, batch, &mut report).await?;
    }

    let pulled = hub.pull(store.pull_cursor().map_err(other)?).await?;
    report.received_rules = store.merge_rules(&pulled.rules).map_err(other)?;
    report.received_words = store.merge_words(&pulled.words).map_err(other)?;
    report.received_snippets = store.merge_snippets(&pulled.snippets).map_err(other)?;
    let conn = store.conn().map_err(other)?;
    set_setting(&conn, "pull_cursor", &pulled.cursor.to_string()).map_err(other)?;
    set_setting(&conn, "last_sync_at", &now_ms().to_string()).map_err(other)?;
    Ok(report)
}

/// Pushes a batch. When the hub refuses it for good, sends its records one
/// by one and sets aside only those it refuses, so one bad record does not
/// block the rest forever. Returns the number of pushes made.
async fn push_or_set_aside(
    store: &LearningStore,
    hub: &impl HubApi,
    batch: PushRequest,
    report: &mut SyncReport,
) -> Result<usize, HubError> {
    let other = |e: anyhow::Error| HubError::Other(format!("{e:#}"));
    let why = match hub.push(&batch).await {
        Ok(response) => {
            let batch = accepted(batch, &response, report);
            store.mark_sent(&batch).map_err(other)?;
            report.count(&batch);
            return Ok(1);
        }
        Err(HubError::Rejected(why)) => why,
        Err(e) => return Err(e),
    };
    let singles = batch.split();
    if singles.len() == 1 {
        log::warn!("The hub refused a record; it is set aside: {why}");
        store.set_aside(&batch, &why).map_err(other)?;
        report.refused += 1;
        return Ok(1);
    }
    let mut pushes = 1;
    for single in singles {
        pushes += 1;
        match hub.push(&single).await {
            Ok(response) => {
                let single = accepted(single, &response, report);
                store.mark_sent(&single).map_err(other)?;
                report.count(&single);
            }
            Err(HubError::Rejected(why)) => {
                log::warn!("The hub refused a record; it is set aside: {why}");
                store.set_aside(&single, &why).map_err(other)?;
                report.refused += 1;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(pushes)
}

/// What of a pushed batch the hub really stored. A hub that predates
/// custom words answers without a `words` count: its words stay queued, and
/// the rest of this run sends none.
fn accepted(
    mut batch: PushRequest,
    response: &PushResponse,
    report: &mut SyncReport,
) -> PushRequest {
    if response.words.is_none() && !batch.words.is_empty() {
        if !report.words_unsupported {
            log::warn!("The hub does not store custom words yet; update it to sync them");
        }
        report.words_unsupported = true;
        batch.words.clear();
    }
    if response.snippets.is_none() && !batch.snippets.is_empty() {
        if !report.snippets_unsupported {
            log::warn!("The hub does not store snippets yet; update it to sync them");
        }
        report.snippets_unsupported = true;
        batch.snippets.clear();
    }
    batch
}

impl SyncReport {
    fn count(&mut self, batch: &PushRequest) {
        self.sent_dictations += batch.dictations.len();
        self.sent_rules += batch.rules.len();
    }
}

/// A batch cut into pushes of one record each.
trait Split {
    fn split(&self) -> Vec<PushRequest>;
}

impl Split for PushRequest {
    fn split(&self) -> Vec<PushRequest> {
        let one = PushRequest::default;
        self.dictations
            .iter()
            .map(|d| PushRequest {
                dictations: vec![d.clone()],
                ..one()
            })
            .chain(self.rules.iter().map(|r| PushRequest {
                rules: vec![r.clone()],
                ..one()
            }))
            .chain(self.substitutions.iter().map(|x| PushRequest {
                substitutions: vec![x.clone()],
                ..one()
            }))
            .chain(self.meetings.iter().map(|m| PushRequest {
                meetings: vec![m.clone()],
                ..one()
            }))
            .chain(self.words.iter().map(|w| PushRequest {
                words: vec![w.clone()],
                ..one()
            }))
            .chain(self.snippets.iter().map(|x| PushRequest {
                snippets: vec![x.clone()],
                ..one()
            }))
            .collect()
    }
}

// ------------------------------------------------------------ HTTP client --

/// The real hub, over HTTP.
pub struct HttpHub {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

/// The hub's lock state, as `GET /api/status` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum HubLockState {
    Uninitialized,
    Locked,
    Unlocked,
}

#[derive(Deserialize)]
struct StatusBody {
    state: HubLockState,
}

#[derive(Deserialize)]
struct RegisterBody {
    token: String,
}

/// Checks and normalizes a hub address typed by the user.
pub fn normalize_url(url: &str) -> Result<String, HubError> {
    let url = url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| HubError::Rejected("the address is not a valid URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(HubError::Rejected(
            "the address must start with http:// or https://".into(),
        ));
    }
    Ok(url.to_string())
}

impl HttpHub {
    pub fn new(base: &str, token: Option<&str>) -> Result<Self, HubError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Hikaya/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HubError::Other(e.to_string()))?;
        Ok(Self {
            base: normalize_url(base)?,
            token: token.map(str::to_string),
            http,
        })
    }

    pub fn for_config(config: &HubConfig) -> Result<Self, HubError> {
        Self::new(&config.url, Some(&config.token))
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, HubError> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|e| HubError::Offline(e.without_url().to_string()))?;
        Self::read(response).await
    }

    /// The JSON body of a successful answer, or the error it reports.
    async fn read<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T, HubError> {
        if response.status().is_success() {
            return response
                .json()
                .await
                .map_err(|e| HubError::Other(format!("unexpected answer from the hub: {e}")));
        }
        Err(Self::error(response).await)
    }

    /// The error an unsuccessful answer reports.
    async fn error(response: reqwest::Response) -> HubError {
        let status = response.status();
        let message = response
            .json::<ErrorBody>()
            .await
            .ok()
            .and_then(|b| b.message)
            .unwrap_or_else(|| status.to_string());
        match status.as_u16() {
            423 => HubError::Locked,
            401 => HubError::Unauthorized,
            400..=499 => HubError::Rejected(message),
            _ => HubError::Other(message),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn status(&self) -> Result<HubLockState, HubError> {
        let body: StatusBody = self.send(self.http.get(self.url("/api/status"))).await?;
        Ok(body.state)
    }

    /// Creates the hub's vault. Only works once, on a new hub.
    pub async fn initialize(&self, passphrase: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url("/api/init"))
                    .json(&serde_json::json!({ "passphrase": passphrase })),
            )
            .await?;
        Ok(())
    }

    pub async fn unlock(&self, passphrase: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url("/api/unlock"))
                    .json(&serde_json::json!({ "passphrase": passphrase })),
            )
            .await?;
        Ok(())
    }

    pub async fn register(&self, passphrase: &str, name: &str) -> Result<String, HubError> {
        let body: RegisterBody = self
            .send(
                self.http
                    .post(self.url("/api/devices"))
                    .json(&serde_json::json!({ "passphrase": passphrase, "name": name })),
            )
            .await?;
        Ok(body.token)
    }
}

/// A recording made on a phone, waiting to be transcribed (ADR 0004).
#[derive(Clone, Debug, Deserialize)]
pub struct PhoneRecording {
    pub id: String,
    pub title: String,
    pub mime: String,
    /// Unix ms.
    pub created_at: i64,
    pub bytes: i64,
}

/// Downloading a long recording takes longer than a sync request.
const AUDIO_TIMEOUT: Duration = Duration::from_secs(900);

impl HttpHub {
    /// Takes the oldest recording waiting for a computer, if any.
    pub async fn claim_recording(&self) -> Result<Option<PhoneRecording>, HubError> {
        let request = self.http.post(self.url("/api/recordings/claim"));
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|e| HubError::Offline(e.without_url().to_string()))?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Self::read(response).await.map(Some)
    }

    /// The claimed recording's audio file.
    pub async fn recording_audio(&self, id: &str) -> Result<Vec<u8>, HubError> {
        let mut request = self
            .http
            .get(self.url(&format!("/api/recordings/{id}/audio")))
            .timeout(AUDIO_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| HubError::Offline(e.without_url().to_string()))?;
        if !response.status().is_success() {
            return Err(Self::error(response).await);
        }
        Ok(response
            .bytes()
            .await
            .map_err(|e| HubError::Offline(e.without_url().to_string()))?
            .to_vec())
    }

    /// Tells the hub the recording became `meeting_uid`; it deletes the audio.
    pub async fn complete_recording(&self, id: &str, meeting_uid: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url(&format!("/api/recordings/{id}/complete")))
                    .json(&serde_json::json!({ "meeting_id": meeting_uid })),
            )
            .await?;
        Ok(())
    }

    /// Gives a recording back after a failure, with the reason.
    pub async fn fail_recording(&self, id: &str, error: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url(&format!("/api/recordings/{id}/fail")))
                    .json(&serde_json::json!({ "error": error })),
            )
            .await?;
        Ok(())
    }
}

impl HubApi for HttpHub {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, HubError> {
        let timeout = if request.meetings.is_empty() {
            REQUEST_TIMEOUT
        } else {
            MEETING_TIMEOUT
        };
        self.send(
            self.http
                .post(self.url("/api/sync/push"))
                .timeout(timeout)
                .json(request),
        )
        .await
    }

    async fn pull(&self, since: i64) -> Result<PullResponse, HubError> {
        self.send(
            self.http
                .get(self.url("/api/sync/pull"))
                .query(&[("since", since)]),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An in-memory hub with the same rules as the real one.
    #[derive(Default)]
    struct FakeHub {
        dictations: Mutex<Vec<DictationRecord>>,
        meetings: Mutex<Vec<MeetingRecord>>,
        rules: Mutex<Vec<(i64, RuleRecord)>>,
        /// Every word the hub holds; a pull returns all of them, which the
        /// merge must tolerate anyway.
        words: Mutex<Vec<WordRecord>>,
        /// Answers like a hub that predates custom words, and keeps none.
        old: Mutex<bool>,
        /// Every snippet the hub holds; a pull returns all of them.
        snippets: Mutex<Vec<SnippetRecord>>,
        error: Mutex<Option<HubError>>,
        pushes: Mutex<usize>,
        /// Ids the hub refuses, as the real one refuses an invalid record:
        /// the whole push fails.
        refuse: Mutex<Vec<String>>,
    }

    impl FakeHub {
        fn fail_with(&self, error: Option<HubError>) {
            *self.error.lock().expect("lock") = error;
        }
        fn texts(&self) -> Vec<String> {
            self.dictations
                .lock()
                .expect("lock")
                .iter()
                .map(|d| d.text.clone())
                .collect()
        }
        fn add_remote_rule(&self, rule: RuleRecord) {
            let mut rules = self.rules.lock().expect("lock");
            let seq = rules.len() as i64 + 1;
            rules.retain(|(_, r)| r.id != rule.id);
            rules.push((seq, rule));
        }
    }

    impl HubApi for FakeHub {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, HubError> {
            if let Some(error) = self.error.lock().expect("lock").clone() {
                return Err(error);
            }
            *self.pushes.lock().expect("lock") += 1;
            let refused = self.refuse.lock().expect("lock").clone();
            let ids = request
                .dictations
                .iter()
                .map(|d| &d.id)
                .chain(request.meetings.iter().map(|m| &m.id))
                .chain(request.rules.iter().map(|r| &r.id))
                .chain(request.words.iter().map(|w| &w.word))
                .chain(request.snippets.iter().map(|x| &x.id));
            if let Some(id) = ids.into_iter().find(|id| refused.contains(id)) {
                return Err(HubError::Rejected(format!("record {id} is invalid")));
            }
            let mut dictations = self.dictations.lock().expect("lock");
            for d in &request.dictations {
                match dictations.iter_mut().find(|x| x.id == d.id) {
                    Some(x) if x.updated_at < d.updated_at => *x = d.clone(),
                    Some(_) => {}
                    None => dictations.push(d.clone()),
                }
            }
            drop(dictations);
            let mut meetings = self.meetings.lock().expect("lock");
            for m in &request.meetings {
                match meetings.iter_mut().find(|x| x.id == m.id) {
                    Some(x) if x.updated_at < m.updated_at => *x = m.clone(),
                    Some(_) => {}
                    None => meetings.push(m.clone()),
                }
            }
            drop(meetings);
            for r in &request.rules {
                let newer = self
                    .rules
                    .lock()
                    .expect("lock")
                    .iter()
                    .find(|(_, x)| x.id == r.id)
                    .is_none_or(|(_, x)| x.updated_at < r.updated_at);
                if newer {
                    self.add_remote_rule(r.clone());
                }
            }
            let old = *self.old.lock().expect("lock");
            let mut words = self.words.lock().expect("lock");
            for w in request.words.iter().filter(|_| !old) {
                match words.iter_mut().find(|x| x.word == w.word) {
                    Some(x) if x.updated_at < w.updated_at => *x = w.clone(),
                    Some(_) => {}
                    None => words.push(w.clone()),
                }
            }
            if old {
                return Ok(PushResponse::default());
            }
            let mut snippets = self.snippets.lock().expect("lock");
            for x in &request.snippets {
                match snippets.iter_mut().find(|y| y.id == x.id) {
                    Some(y) if y.updated_at < x.updated_at => *y = x.clone(),
                    Some(_) => {}
                    None => snippets.push(x.clone()),
                }
            }
            Ok(PushResponse {
                words: Some(request.words.len()),
                snippets: Some(request.snippets.len()),
                ..Default::default()
            })
        }

        async fn pull(&self, since: i64) -> Result<PullResponse, HubError> {
            if let Some(error) = self.error.lock().expect("lock").clone() {
                return Err(error);
            }
            let rules = self.rules.lock().expect("lock");
            let changed: Vec<&(i64, RuleRecord)> =
                rules.iter().filter(|(s, _)| *s > since).collect();
            Ok(PullResponse {
                cursor: changed.iter().map(|(s, _)| *s).max().unwrap_or(since),
                rules: changed.into_iter().map(|(_, r)| r.clone()).collect(),
                words: self.words.lock().expect("lock").clone(),
                snippets: self.snippets.lock().expect("lock").clone(),
            })
        }
    }

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open");
        (dir, store)
    }

    fn connected() -> (tempfile::TempDir, LearningStore) {
        let (dir, store) = store();
        store
            .save_hub_config("http://hub.test", "hky_token", "laptop")
            .expect("connect");
        (dir, store)
    }

    fn remote_rule(id: &str, at: i64, pattern: &str, replacement: &str) -> RuleRecord {
        RuleRecord {
            id: id.into(),
            pattern: pattern.into(),
            replacement: replacement.into(),
            enabled: true,
            source: "manual".into(),
            updated_at: at,
            deleted: false,
        }
    }

    #[tokio::test]
    async fn nothing_is_queued_until_a_hub_is_connected() {
        let (_dir, store) = store();
        assert!(!store
            .queue_dictation(1, 1000, "bonjour", Some("fr"))
            .expect("queue"));
        assert_eq!(store.pending_count().expect("count"), 0);
        assert_eq!(store.hub_config().expect("config"), None);
    }

    #[tokio::test]
    async fn queued_dictations_reach_the_hub_and_leave_the_queue() {
        let (_dir, store) = connected();
        assert!(store
            .queue_dictation(1, 1000, "bonjour", Some("fr"))
            .expect("queue"));
        store
            .queue_dictation(2, 2000, "hello", None)
            .expect("queue");
        assert_eq!(store.pending_count().expect("count"), 2);

        let hub = FakeHub::default();
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.sent_dictations, 2);
        assert_eq!(hub.texts(), ["bonjour", "hello"]);
        assert_eq!(store.pending_count().expect("count"), 0);
        assert!(store.last_sync_at().expect("last").is_some());
    }

    #[tokio::test]
    async fn a_locked_or_unreachable_hub_keeps_the_queue() {
        let (_dir, store) = connected();
        store
            .queue_dictation(1, 1000, "bonjour", None)
            .expect("queue");
        let hub = FakeHub::default();
        for error in [HubError::Locked, HubError::Offline("down".into())] {
            hub.fail_with(Some(error.clone()));
            assert_eq!(sync_once(&store, &hub).await, Err(error));
            assert_eq!(store.pending_count().expect("count"), 1);
        }
        hub.fail_with(None);
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts(), ["bonjour"]);
    }

    #[tokio::test]
    async fn a_correction_is_sent_as_a_newer_version_of_the_same_dictation() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store
            .queue_dictation(7, 1000, "merci a med", None)
            .expect("queue");
        sync_once(&store, &hub).await.expect("sync");
        std::thread::sleep(Duration::from_millis(2));
        store
            .queue_dictation(7, 1000, "merci Ahmed", None)
            .expect("correct");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts(), ["merci Ahmed"]);
    }

    #[tokio::test]
    async fn rules_go_up_and_come_down_with_the_newest_change_winning() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let local = store.add_rule("get hub", "GitHub", "manual").expect("add");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.sent_rules, 1);

        // Another device adds a rule and edits ours later.
        hub.add_remote_rule(remote_rule("phone-1", now_ms(), "clod", "Claude"));
        let ours = hub.rules.lock().expect("lock")[0].1.clone();
        hub.add_remote_rule(RuleRecord {
            replacement: "GitHub.com".into(),
            updated_at: local.updated_at + 10_000,
            ..ours
        });
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.received_rules, 2);
        let mut rules: Vec<(String, String)> = store
            .list_rules()
            .expect("list")
            .into_iter()
            .map(|r| (r.pattern, r.replacement))
            .collect();
        rules.sort();
        assert_eq!(
            rules,
            [
                ("clod".to_string(), "Claude".to_string()),
                ("get hub".to_string(), "GitHub.com".to_string())
            ]
        );
        // Received rules are applied to the next dictation.
        assert_eq!(store.active_rules().len(), 2);
        assert_eq!(store.pending_count().expect("count"), 0);
    }

    #[tokio::test]
    async fn an_older_remote_change_does_not_overwrite_a_newer_local_one() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let local = store.add_rule("clod", "Claude", "manual").expect("add");
        let uid: String = store
            .conn()
            .expect("conn")
            .query_row("SELECT uid FROM rules", [], |r| r.get(0))
            .expect("uid");
        hub.add_remote_rule(remote_rule(&uid, local.updated_at - 5_000, "clod", "cloud"));
        // Pull happens after push in the same run; the hub keeps the newer one.
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(store.list_rules().expect("list")[0].replacement, "Claude");
    }

    #[tokio::test]
    async fn a_deletion_reaches_the_hub_and_the_tombstone_is_then_dropped() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let rule = store.add_rule("clod", "Claude", "manual").expect("add");
        sync_once(&store, &hub).await.expect("sync");
        std::thread::sleep(Duration::from_millis(2));
        store.delete_rule(rule.id).expect("delete");
        assert!(store.list_rules().expect("list").is_empty());
        sync_once(&store, &hub).await.expect("sync");
        assert!(hub.rules.lock().expect("lock")[0].1.deleted);
        let left: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM rules", [], |r| r.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_remote_deletion_removes_the_local_rule() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store.add_rule("clod", "Claude", "manual").expect("add");
        sync_once(&store, &hub).await.expect("sync");
        let mut r = hub.rules.lock().expect("lock")[0].1.clone();
        r.deleted = true;
        r.updated_at += 10_000;
        hub.add_remote_rule(r);
        sync_once(&store, &hub).await.expect("sync");
        assert!(store.list_rules().expect("list").is_empty());
        assert!(store.active_rules().is_empty());
    }

    #[tokio::test]
    async fn a_large_backlog_is_sent_in_several_pushes() {
        let (_dir, store) = connected();
        for id in 0..450 {
            store
                .queue_dictation(id, 1000 + id, &format!("dictée {id}"), None)
                .expect("queue");
        }
        let hub = FakeHub::default();
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts().len(), 450);
        assert_eq!(*hub.pushes.lock().expect("lock"), 3);
        assert!(!sync_once(&store, &hub).await.expect("sync").more);
        assert_eq!(store.pending_count().expect("count"), 0);
    }

    #[tokio::test]
    async fn a_refused_dictation_is_set_aside_and_the_others_still_go() {
        let (_dir, store) = connected();
        for id in 1..=3 {
            store
                .queue_dictation(id, 1000 + id, &format!("dictée {id}"), None)
                .expect("queue");
        }
        store.add_rule("clod", "Claude", "manual").expect("rule");
        let uid = store.hub_config().expect("c").expect("c").device_uid + "-2";
        let hub = FakeHub::default();
        hub.refuse.lock().expect("lock").push(uid);

        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.refused, 1);
        assert_eq!(hub.texts(), ["dictée 1", "dictée 3"]);
        assert_eq!(
            hub.rules.lock().expect("lock").len(),
            1,
            "the rule got through"
        );
        assert_eq!(store.pending_count().expect("pending"), 0);
        assert_eq!(store.refused_count().expect("refused"), 1);

        // Nothing is stuck: the next run has nothing to send.
        let pushes = *hub.pushes.lock().expect("lock");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(*hub.pushes.lock().expect("lock"), pushes);
    }

    #[tokio::test]
    async fn a_hub_that_is_down_is_not_mistaken_for_a_refusal() {
        let (_dir, store) = connected();
        store
            .queue_dictation(1, 1000, "bonjour", None)
            .expect("queue");
        let hub = FakeHub::default();
        hub.fail_with(Some(HubError::Other("502 Bad Gateway".into())));
        assert!(sync_once(&store, &hub).await.is_err());
        assert_eq!(store.pending_count().expect("pending"), 1);
        assert_eq!(store.refused_count().expect("refused"), 0);
    }

    #[tokio::test]
    async fn a_run_at_its_push_limit_says_there_is_more() {
        let (_dir, store) = connected();
        let total = (MAX_PUSHES_PER_RUN * DICTATIONS_PER_PUSH + 5) as i64;
        for id in 0..total {
            store
                .queue_dictation(id, 1000 + id, &format!("d{id}"), None)
                .expect("queue");
        }
        let hub = FakeHub::default();
        let first = sync_once(&store, &hub).await.expect("sync");
        assert!(first.more);
        let second = sync_once(&store, &hub).await.expect("sync");
        assert!(!second.more);
        assert_eq!(hub.texts().len(), total as usize);
    }

    #[tokio::test]
    async fn a_new_hub_receives_rules_and_meetings_again() {
        use crate::hikaya::meeting::store::MeetingStatus;
        let (_dir, store) = connected();
        store.add_rule("clod", "Claude", "manual").expect("rule");
        let id = store
            .create_meeting("Point", 1_000, false)
            .expect("meeting");
        store
            .set_meeting_status(id, MeetingStatus::Done, 1_000)
            .expect("done");
        sync_once(&store, &FakeHub::default()).await.expect("sync");
        assert_eq!(store.pending_count().expect("pending"), 0);

        // Same hub again (a new token): nothing to resend.
        store
            .save_hub_config("http://hub.test", "hky_other", "laptop")
            .expect("reconnect");
        assert_eq!(store.pending_count().expect("pending"), 0);

        store
            .save_hub_config("http://new-hub.test", "hky_new", "laptop")
            .expect("switch");
        assert_eq!(store.pending_count().expect("pending"), 2);
        let new_hub = FakeHub::default();
        sync_once(&store, &new_hub).await.expect("sync");
        assert_eq!(new_hub.rules.lock().expect("lock").len(), 1);
        assert_eq!(new_hub.meetings.lock().expect("lock").len(), 1);
    }

    #[tokio::test]
    async fn disconnecting_empties_the_queue_but_keeps_rules() {
        let (_dir, store) = connected();
        store.add_rule("clod", "Claude", "manual").expect("add");
        store
            .queue_dictation(1, 1000, "bonjour", None)
            .expect("queue");
        store.clear_hub_config().expect("disconnect");
        assert_eq!(store.hub_config().expect("config"), None);
        assert_eq!(store.list_rules().expect("list").len(), 1);
        let queued: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM sync_outbox", [], |r| r.get(0))
            .expect("count");
        assert_eq!(queued, 0);
    }

    #[tokio::test]
    async fn a_finished_meeting_is_sent_whole_and_again_when_it_changes() {
        use crate::hikaya::meeting::store::MeetingStatus;
        use hikaya_core::meeting::{Speaker, TranscriptLine};
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let id = store
            .create_meeting("Point produit", 1_000, true)
            .expect("create");
        for (start, speaker, text) in [
            (0, Speaker::Others, "On décale ?"),
            (30_000, Speaker::You, "Oui, jeudi."),
        ] {
            store
                .add_meeting_line(
                    id,
                    &TranscriptLine {
                        start_ms: start,
                        end_ms: start + 30_000,
                        speaker,
                        text: text.into(),
                        voice: None,
                    },
                )
                .expect("line");
        }
        // Still recording: not sent.
        sync_once(&store, &hub).await.expect("sync");
        assert!(hub.meetings.lock().expect("lock").is_empty());

        store
            .set_meeting_status(id, MeetingStatus::Done, 60_000)
            .expect("done");
        assert_eq!(store.pending_count().expect("pending"), 1);
        sync_once(&store, &hub).await.expect("sync");
        {
            let sent = hub.meetings.lock().expect("lock");
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].title, "Point produit");
            assert_eq!(sent[0].lines.len(), 2);
            assert_eq!(sent[0].lines[0].speaker, "others");
        }
        assert_eq!(store.pending_count().expect("pending"), 0);

        std::thread::sleep(Duration::from_millis(2));
        store
            .set_meeting_summary(id, "## TL;DR\nDémo jeudi.", "test")
            .expect("summary");
        assert_eq!(store.pending_count().expect("pending"), 1);
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(
            hub.meetings.lock().expect("lock")[0].summary.as_deref(),
            Some("## TL;DR\nDémo jeudi.")
        );
    }

    #[test]
    fn hub_addresses_are_checked() {
        assert_eq!(
            normalize_url(" https://hub.example.ts.net/ ").expect("ok"),
            "https://hub.example.ts.net"
        );
        assert_eq!(
            normalize_url("http://100.64.0.1:8787").expect("ok"),
            "http://100.64.0.1:8787"
        );
        for bad in [
            "",
            "hub.local",
            "ftp://hub",
            "javascript:alert(1)",
            "http://",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_device_uid_is_stable() {
        let (_dir, store) = connected();
        let first = store
            .hub_config()
            .expect("config")
            .expect("some")
            .device_uid;
        let again = store
            .hub_config()
            .expect("config")
            .expect("some")
            .device_uid;
        assert_eq!(first, again);
        assert_eq!(first.len(), 16);
    }

    fn hub_word(word: &str, at: i64, deleted: bool) -> WordRecord {
        WordRecord {
            word: word.into(),
            updated_at: at,
            deleted,
        }
    }

    fn live_hub_words(hub: &FakeHub) -> Vec<String> {
        let mut words: Vec<String> = hub
            .words
            .lock()
            .expect("lock")
            .iter()
            .filter(|w| !w.deleted)
            .map(|w| w.word.clone())
            .collect();
        words.sort();
        words
    }

    /// What the sync worker does with pulled words: put them in the
    /// settings list, then mark them applied.
    fn apply_pending(store: &LearningStore, list: &mut Vec<String>) -> usize {
        let changes = store.pending_word_changes().expect("pending");
        hikaya_core::sync::apply_word_changes(list, &changes);
        store.mark_words_applied(&changes).expect("applied");
        changes.len()
    }

    #[tokio::test]
    async fn custom_words_added_and_removed_reach_the_hub() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store
            .note_custom_words(&["Hikaya".into(), " Ollama ".into(), "  ".into()])
            .expect("note");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(live_hub_words(&hub), ["Hikaya", "Ollama"]);

        store.note_custom_words(&["Hikaya".into()]).expect("note");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(live_hub_words(&hub), ["Hikaya"]);
        // The hub's own records of what this device sent change nothing here.
        assert_eq!(report.received_words, 0);
        // The tombstone has reached the hub, so it is gone on this device.
        let left: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM custom_words", [], |r| r.get(0))
            .expect("count");
        assert_eq!(left, 1);
        // Nothing changed: nothing is queued again.
        store.note_custom_words(&["Hikaya".into()]).expect("note");
        assert!(store
            .next_batch(true, true)
            .expect("batch")
            .words
            .is_empty());
    }

    #[tokio::test]
    async fn custom_words_from_other_devices_come_back_once() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        hub.words.lock().expect("lock").extend([
            hub_word("Kubernetes", 100, false),
            hub_word("gone", 100, true),
        ]);
        let mut settings: Vec<String> = vec!["Hikaya".into()];
        store.note_custom_words(&settings).expect("note");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.received_words, 1);
        assert_eq!(apply_pending(&store, &mut settings), 1);
        assert_eq!(settings, ["Hikaya", "Kubernetes"]);
        store.note_custom_words(&settings).expect("note");
        let again = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(again.received_words, 0);
        assert!(store
            .next_batch(true, true)
            .expect("batch")
            .words
            .is_empty());

        // Removed here: the removal is sent.
        settings.retain(|w| w != "Kubernetes");
        store.note_custom_words(&settings).expect("note");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(live_hub_words(&hub), ["Hikaya"]);

        // Added again on another device later: it comes back.
        hub.words.lock().expect("lock")[0] = hub_word("Kubernetes", now_ms() + 10_000, false);
        sync_once(&store, &hub).await.expect("sync");
        apply_pending(&store, &mut settings);
        assert_eq!(settings, ["Hikaya", "Kubernetes"]);
    }

    #[tokio::test]
    async fn a_pulled_word_not_yet_in_the_settings_is_not_taken_for_a_removal() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        hub.words
            .lock()
            .expect("lock")
            .push(hub_word("Kubernetes", 100, false));
        store.note_custom_words(&[]).expect("note");
        sync_once(&store, &hub).await.expect("sync");
        // The run stops here (a crash, a failed write): the settings never
        // got the word. The next run must not send a removal for it.
        store.note_custom_words(&[]).expect("note");
        assert!(store
            .next_batch(true, true)
            .expect("batch")
            .words
            .is_empty());
        let mut settings = Vec::new();
        assert_eq!(apply_pending(&store, &mut settings), 1);
        assert_eq!(settings, ["Kubernetes"]);
        assert_eq!(live_hub_words(&hub), ["Kubernetes"]);
    }

    #[tokio::test]
    async fn a_removal_beats_the_seen_version_even_with_a_slow_clock() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        // Another device, its clock an hour ahead, added the word.
        let ahead = now_ms() + 3_600_000;
        hub.words
            .lock()
            .expect("lock")
            .push(hub_word("Kubernetes", ahead, false));
        let mut settings = Vec::new();
        sync_once(&store, &hub).await.expect("sync");
        apply_pending(&store, &mut settings);
        settings.clear();
        store.note_custom_words(&settings).expect("note");
        let batch = store.next_batch(true, true).expect("batch");
        assert!(batch.words[0].deleted);
        assert!(batch.words[0].updated_at > ahead);
        sync_once(&store, &hub).await.expect("sync");
        assert!(live_hub_words(&hub).is_empty());
    }

    #[tokio::test]
    async fn words_stay_queued_for_a_hub_that_predates_them() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        *hub.old.lock().expect("lock") = true;
        let many: Vec<String> = (0..150).map(|i| format!("word{i}")).collect();
        store.note_custom_words(&many).expect("note");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert!(report.words_unsupported);
        // One push, not one per batch of 100 until the run's limit.
        assert_eq!(*hub.pushes.lock().expect("lock"), 1);
        assert_eq!(
            store.next_batch(true, true).expect("batch").words.len(),
            100
        );

        // Once the hub is updated, they all go.
        *hub.old.lock().expect("lock") = false;
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(live_hub_words(&hub).len(), 150);
    }

    #[tokio::test]
    async fn a_refused_word_is_set_aside() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        hub.refuse.lock().expect("lock").push("bad".into());
        store
            .note_custom_words(&["bad".into(), "good".into()])
            .expect("note");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.refused, 1);
        assert_eq!(live_hub_words(&hub), ["good"]);
        assert!(store
            .next_batch(true, true)
            .expect("batch")
            .words
            .is_empty());
        assert_eq!(store.refused_count().expect("count"), 1);
    }

    #[tokio::test]
    async fn snippets_reach_the_hub_and_come_back_from_other_devices() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let saved = store
            .save_snippet("my address", "12 rue de la Paix\n75002 Paris")
            .expect("save");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(
            hub.snippets.lock().expect("lock")[0].text,
            "12 rue de la Paix\n75002 Paris"
        );
        assert!(store
            .next_batch(true, true)
            .expect("batch")
            .snippets
            .is_empty());

        // Changed on another device later: the change arrives here.
        hub.snippets.lock().expect("lock")[0] = SnippetRecord {
            id: saved.id.clone(),
            cue: "my address".into(),
            text: "Elsewhere".into(),
            updated_at: now_ms() + 10_000,
            deleted: false,
        };
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.received_snippets, 1);
        assert_eq!(store.list_snippets().expect("list")[0].text, "Elsewhere");

        // Deleted here: the tombstone is sent, then forgotten.
        store.delete_snippet(&saved.id).expect("delete");
        sync_once(&store, &hub).await.expect("sync");
        assert!(hub.snippets.lock().expect("lock")[0].deleted);
        assert!(store.list_snippets().expect("list").is_empty());
    }

    #[tokio::test]
    async fn snippets_stay_queued_for_a_hub_that_predates_them() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        *hub.old.lock().expect("lock") = true;
        store.save_snippet("sig", "— Ahmed").expect("save");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert!(report.snippets_unsupported);
        assert_eq!(
            store.next_batch(true, true).expect("batch").snippets.len(),
            1
        );
    }

    #[tokio::test]
    async fn the_same_cue_made_on_two_devices_ends_as_one() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store.save_snippet("my address", "Here").expect("save");
        sync_once(&store, &hub).await.expect("sync");
        // Another device made the same cue, later, with its own id.
        hub.snippets.lock().expect("lock").push(SnippetRecord {
            id: "other".into(),
            cue: "My Address".into(),
            text: "There".into(),
            updated_at: now_ms() + 10_000,
            deleted: false,
        });
        sync_once(&store, &hub).await.expect("sync");
        let list = store.list_snippets().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].text, "There");
        // The older one is deleted on the hub too, so every device agrees.
        sync_once(&store, &hub).await.expect("sync");
        let live: Vec<_> = hub
            .snippets
            .lock()
            .expect("lock")
            .iter()
            .filter(|x| !x.deleted)
            .map(|x| x.id.clone())
            .collect();
        assert_eq!(live, ["other"]);
    }
}
