//! Applying what devices push, and serving what they pull.
//!
//! - Dictations: insert, or replace the text when the incoming
//!   `updated_at` is newer (a correction made on the device).
//! - Rules: the newest `updated_at` wins, deletions included. Each accepted
//!   change gets the next sequence number, which is the pull cursor.
//! - Custom words: like rules, keyed by the word itself.
//! - Snippets: like rules.
//! - Substitutions: insert once; they never change.
//!
//! A device can only overwrite records it created. Ids are global, so a
//! device reusing another device's id is refused rather than merged.

use anyhow::{Context, Result};
use hikaya_core::sync::{
    clean_word, DictationRecord, PullResponse, PushRequest, PushResponse, RuleRecord,
    SnippetRecord, WordRecord, MAX_RECORDS_PER_PUSH, MAX_TEXT_BYTES,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

/// Why a push was refused as a whole. Nothing is written when this happens.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("a push may carry at most {MAX_RECORDS_PER_PUSH} records")]
    TooLarge,
    #[error("record {0}: {1}")]
    Invalid(String, String),
    #[error("record {0} belongs to another device")]
    NotYours(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn check_id(id: &str) -> Result<(), PushError> {
    let ok = (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(PushError::Invalid(
            id.chars().take(64).collect(),
            "an id is 1 to 64 letters, digits, '-' or '_'".into(),
        ))
    }
}

fn validate(request: &PushRequest) -> Result<(), PushError> {
    if request.len() > MAX_RECORDS_PER_PUSH {
        return Err(PushError::TooLarge);
    }
    for d in &request.dictations {
        check_id(&d.id)?;
        if d.text.len() > MAX_TEXT_BYTES {
            return Err(PushError::Invalid(
                d.id.clone(),
                "the text is too long".into(),
            ));
        }
        if d.updated_at < d.created_at {
            return Err(PushError::Invalid(
                d.id.clone(),
                "updated_at is before created_at".into(),
            ));
        }
    }
    for r in &request.rules {
        check_id(&r.id)?;
        if r.pattern.trim().is_empty() || r.pattern.len() > 200 || r.replacement.len() > 200 {
            return Err(PushError::Invalid(
                r.id.clone(),
                "a rule needs a pattern, and both sides at most 200 bytes".into(),
            ));
        }
        if r.source != "manual" && r.source != "learned" {
            return Err(PushError::Invalid(
                r.id.clone(),
                "source must be manual or learned".into(),
            ));
        }
    }
    for m in &request.meetings {
        check_id(&m.id)?;
        crate::meetings::validate(m)?;
    }
    for s in &request.snippets {
        check_id(&s.id)?;
        if !s.deleted && !hikaya_core::snippets::valid(&s.cue, &s.text) {
            return Err(PushError::Invalid(
                s.id.clone(),
                "a snippet needs a cue of at most 100 bytes and a text of at most 4000".into(),
            ));
        }
    }
    for w in &request.words {
        if clean_word(&w.word) != Some(w.word.as_str()) {
            return Err(PushError::Invalid(
                w.word.chars().take(64).collect(),
                "a word is trimmed, 1 to 100 bytes, without control characters".into(),
            ));
        }
    }
    for s in &request.substitutions {
        check_id(&s.id)?;
        if s.from.len() > 200 || s.to.len() > 200 {
            return Err(PushError::Invalid(s.id.clone(), "too long".into()));
        }
    }
    Ok(())
}

/// The device that created a record, if the record exists.
fn owner(tx: &Transaction<'_>, table: &str, id: &str) -> Result<Option<i64>> {
    Ok(tx
        .query_row(
            &format!("SELECT device_id FROM {table} WHERE id = ?1"),
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

fn next_seq(tx: &Transaction<'_>) -> Result<i64> {
    tx.query_row(
        "UPDATE sync_counter SET seq = seq + 1 WHERE id = 1 RETURNING seq",
        [],
        |row| row.get(0),
    )
    .context("advancing the sync counter")
}

fn apply_dictation(
    tx: &Transaction<'_>,
    device: i64,
    d: &DictationRecord,
) -> Result<bool, PushError> {
    match owner(tx, "dictations", &d.id)? {
        None => {
            tx.execute(
                "INSERT INTO dictations (id, device_id, created_at, updated_at, text, language, duration_ms, received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s','now') * 1000)",
                params![d.id, device, d.created_at, d.updated_at, d.text, d.language, d.duration_ms],
            )
            .context("saving a dictation")?;
            Ok(true)
        }
        Some(other) if other != device => Err(PushError::NotYours(d.id.clone())),
        Some(_) => {
            let changed = tx
                .execute(
                    "UPDATE dictations SET text = ?2, updated_at = ?3, language = ?4, duration_ms = ?5
                     WHERE id = ?1 AND updated_at < ?3",
                    params![d.id, d.text, d.updated_at, d.language, d.duration_ms],
                )
                .context("updating a dictation")?;
            Ok(changed > 0)
        }
    }
}

fn apply_rule(tx: &Transaction<'_>, device: i64, r: &RuleRecord) -> Result<bool, PushError> {
    let current: Option<(i64, i64)> = tx
        .query_row(
            "SELECT device_id, updated_at FROM rules WHERE id = ?1",
            params![r.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("reading a rule")?;
    // Rules are shared settings: any of the user's devices may change any
    // rule, and the newest change wins.
    if let Some((_, updated_at)) = current {
        if updated_at >= r.updated_at {
            return Ok(false);
        }
    }
    let seq = next_seq(tx)?;
    tx.execute(
        "INSERT INTO rules (id, device_id, pattern, replacement, enabled, source, updated_at, deleted, seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET device_id = ?2, pattern = ?3, replacement = ?4, enabled = ?5,
           source = ?6, updated_at = ?7, deleted = ?8, seq = ?9",
        params![r.id, device, r.pattern, r.replacement, r.enabled, r.source, r.updated_at, r.deleted, seq],
    )
    .context("saving a rule")?;
    Ok(true)
}

fn apply_word(tx: &Transaction<'_>, device: i64, w: &WordRecord) -> Result<bool, PushError> {
    let current: Option<i64> = tx
        .query_row(
            "SELECT updated_at FROM words WHERE word = ?1",
            params![w.word],
            |row| row.get(0),
        )
        .optional()
        .context("reading a word")?;
    // Shared like rules: the newest change wins, whichever device made it.
    if current.is_some_and(|updated_at| updated_at >= w.updated_at) {
        return Ok(false);
    }
    let seq = next_seq(tx)?;
    tx.execute(
        "INSERT INTO words (word, device_id, updated_at, deleted, seq) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (word) DO UPDATE SET device_id = ?2, updated_at = ?3, deleted = ?4, seq = ?5",
        params![w.word, device, w.updated_at, w.deleted, seq],
    )
    .context("saving a word")?;
    Ok(true)
}

fn apply_snippet(tx: &Transaction<'_>, device: i64, s: &SnippetRecord) -> Result<bool, PushError> {
    let current: Option<i64> = tx
        .query_row(
            "SELECT updated_at FROM snippets WHERE id = ?1",
            params![s.id],
            |row| row.get(0),
        )
        .optional()
        .context("reading a snippet")?;
    // Shared like rules: the newest change wins, whichever device made it.
    if current.is_some_and(|updated_at| updated_at >= s.updated_at) {
        return Ok(false);
    }
    let seq = next_seq(tx)?;
    tx.execute(
        "INSERT INTO snippets (id, device_id, cue, text, updated_at, deleted, seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET device_id = ?2, cue = ?3, text = ?4, updated_at = ?5,
           deleted = ?6, seq = ?7",
        params![s.id, device, s.cue, s.text, s.updated_at, s.deleted, seq],
    )
    .context("saving a snippet")?;
    Ok(true)
}

/// Applies a push from `device` in one transaction: all of it or none of it.
pub fn push(
    conn: &mut Connection,
    device: i64,
    request: &PushRequest,
) -> Result<PushResponse, PushError> {
    validate(request)?;
    let tx = conn.transaction().context("starting the push")?;
    let mut response = PushResponse {
        // Always sent, so a device can tell this hub stores custom words and
        // snippets.
        words: Some(0),
        snippets: Some(0),
        ..Default::default()
    };
    for d in &request.dictations {
        response.dictations += usize::from(apply_dictation(&tx, device, d)?);
    }
    for r in &request.rules {
        response.rules += usize::from(apply_rule(&tx, device, r)?);
    }
    for s in &request.substitutions {
        match owner(&tx, "substitutions", &s.id)? {
            Some(other) if other != device => return Err(PushError::NotYours(s.id.clone())),
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO substitutions (id, device_id, from_text, to_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![s.id, device, s.from, s.to, s.created_at],
                )
                .context("saving a substitution")?;
                response.substitutions += 1;
            }
        }
    }
    for m in &request.meetings {
        response.meetings += usize::from(crate::meetings::apply(&tx, device, m)?);
    }
    for w in &request.words {
        *response.words.get_or_insert(0) += usize::from(apply_word(&tx, device, w)?);
    }
    for s in &request.snippets {
        *response.snippets.get_or_insert(0) += usize::from(apply_snippet(&tx, device, s)?);
    }
    tx.commit().context("committing the push")?;
    Ok(response)
}

/// Rule and word changes after `since`, oldest first, and the cursor to
/// send next. Both share one sequence, so one cursor covers them.
pub fn pull(conn: &Connection, since: i64) -> Result<PullResponse> {
    let mut statement = conn.prepare(
        "SELECT id, pattern, replacement, enabled, source, updated_at, deleted, seq
         FROM rules WHERE seq > ?1 ORDER BY seq",
    )?;
    let mut cursor = since;
    let rules = statement
        .query_map(params![since], |row| {
            Ok((
                RuleRecord {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                    replacement: row.get(2)?,
                    enabled: row.get(3)?,
                    source: row.get(4)?,
                    updated_at: row.get(5)?,
                    deleted: row.get(6)?,
                },
                row.get::<_, i64>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(rule, seq)| {
            cursor = cursor.max(seq);
            rule
        })
        .collect();
    let mut statement = conn
        .prepare("SELECT word, updated_at, deleted, seq FROM words WHERE seq > ?1 ORDER BY seq")?;
    let words = statement
        .query_map(params![since], |row| {
            Ok((
                WordRecord {
                    word: row.get(0)?,
                    updated_at: row.get(1)?,
                    deleted: row.get(2)?,
                },
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(word, seq)| {
            cursor = cursor.max(seq);
            word
        })
        .collect();
    let mut statement = conn.prepare(
        "SELECT id, cue, text, updated_at, deleted, seq FROM snippets WHERE seq > ?1 ORDER BY seq",
    )?;
    let snippets = statement
        .query_map(params![since], |row| {
            Ok((
                SnippetRecord {
                    id: row.get(0)?,
                    cue: row.get(1)?,
                    text: row.get(2)?,
                    updated_at: row.get(3)?,
                    deleted: row.get(4)?,
                },
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(snippet, seq)| {
            cursor = cursor.max(seq);
            snippet
        })
        .collect();
    Ok(PullResponse {
        rules,
        words,
        snippets,
        cursor,
    })
}

/// Turns user input into an FTS5 query of quoted word prefixes, so the
/// input can never use FTS syntax. Empty when there is no word.
pub fn fts_words(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One dictation as listed or found by search.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DictationView {
    pub id: String,
    pub device: String,
    pub created_at: i64,
    pub text: String,
    pub language: Option<String>,
}

/// Newest dictations first, optionally only those matching `query`.
/// The query is matched as words (accents and case ignored), never as
/// FTS syntax, so user input cannot break or widen the search.
pub fn dictations(
    conn: &Connection,
    query: Option<&str>,
    before: Option<i64>,
    limit: usize,
) -> Result<Vec<DictationView>> {
    let before = before.unwrap_or(i64::MAX);
    let limit = limit.clamp(1, 200) as i64;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(DictationView {
            id: row.get(0)?,
            device: row.get(1)?,
            created_at: row.get(2)?,
            text: row.get(3)?,
            language: row.get(4)?,
        })
    };
    let words = fts_words(query.unwrap_or(""));
    let rows = if words.is_empty() {
        conn.prepare(
            "SELECT d.id, v.name, d.created_at, d.text, d.language FROM dictations d
             JOIN devices v ON v.id = d.device_id
             WHERE d.created_at < ?1 ORDER BY d.created_at DESC LIMIT ?2",
        )?
        .query_map(params![before, limit], map)?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        conn.prepare(
            "SELECT d.id, v.name, d.created_at, d.text, d.language FROM dictations_fts f
             JOIN dictations d ON d.rowid = f.rowid
             JOIN devices v ON v.id = d.device_id
             WHERE dictations_fts MATCH ?1 AND d.created_at < ?2
             ORDER BY d.created_at DESC LIMIT ?3",
        )?
        .query_map(params![words, before, limit], map)?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, devices};
    use hikaya_core::sync::SubstitutionRecord;

    fn setup() -> (Connection, i64, i64) {
        let mut conn = Connection::open_in_memory().expect("open");
        db::migrate(&mut conn).expect("migrate");
        let (laptop, _) = devices::register(&conn, "laptop").expect("laptop");
        let (phone, _) = devices::register(&conn, "phone").expect("phone");
        (conn, laptop.id, phone.id)
    }

    fn dictation(id: &str, at: i64, text: &str) -> DictationRecord {
        DictationRecord {
            id: id.into(),
            created_at: at,
            updated_at: at,
            text: text.into(),
            language: Some("fr".into()),
            duration_ms: Some(3000),
        }
    }

    fn rule(id: &str, at: i64, pattern: &str, replacement: &str) -> RuleRecord {
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

    fn texts(conn: &Connection, query: Option<&str>) -> Vec<String> {
        dictations(conn, query, None, 50)
            .expect("list")
            .into_iter()
            .map(|d| d.text)
            .collect()
    }

    #[test]
    fn pushing_twice_stores_once() {
        let (mut conn, laptop, _) = setup();
        let request = PushRequest {
            dictations: vec![dictation("d1", 10, "bonjour")],
            ..Default::default()
        };
        assert_eq!(
            push(&mut conn, laptop, &request).expect("push").dictations,
            1
        );
        assert_eq!(
            push(&mut conn, laptop, &request).expect("again").dictations,
            0
        );
        assert_eq!(texts(&conn, None), ["bonjour"]);
    }

    #[test]
    fn a_newer_correction_replaces_the_text_and_an_older_one_does_not() {
        let (mut conn, laptop, _) = setup();
        let mut d = dictation("d1", 10, "merci a med");
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![d.clone()],
                ..Default::default()
            },
        )
        .expect("push");
        d.text = "merci Ahmed".into();
        d.updated_at = 20;
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![d.clone()],
                ..Default::default()
            },
        )
        .expect("newer");
        d.text = "stale".into();
        d.updated_at = 15;
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![d],
                ..Default::default()
            },
        )
        .expect("older");
        assert_eq!(texts(&conn, None), ["merci Ahmed"]);
        // The search index follows the correction.
        assert_eq!(texts(&conn, Some("ahmed")), ["merci Ahmed"]);
        assert!(texts(&conn, Some("med")).is_empty());
    }

    #[test]
    fn a_device_cannot_overwrite_another_devices_dictation() {
        let (mut conn, laptop, phone) = setup();
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![dictation("d1", 10, "mine")],
                ..Default::default()
            },
        )
        .expect("push");
        let mut theirs = dictation("d1", 10, "hijacked");
        theirs.updated_at = 99;
        let result = push(
            &mut conn,
            phone,
            &PushRequest {
                dictations: vec![dictation("d2", 11, "ok"), theirs],
                ..Default::default()
            },
        );
        assert!(matches!(result, Err(PushError::NotYours(_))));
        // All or nothing: d2 was not kept either.
        assert_eq!(texts(&conn, None), ["mine"]);
    }

    #[test]
    fn rules_newest_wins_and_flow_to_other_devices() {
        let (mut conn, laptop, phone) = setup();
        push(
            &mut conn,
            laptop,
            &PushRequest {
                rules: vec![rule("r1", 100, "get hub", "GitHub")],
                ..Default::default()
            },
        )
        .expect("push");
        let first = pull(&conn, 0).expect("pull");
        assert_eq!(first.rules.len(), 1);
        assert_eq!(
            pull(&conn, first.cursor).expect("nothing new").rules.len(),
            0
        );

        // The phone edits it later; an even older edit from the laptop loses.
        let mut edited = rule("r1", 200, "get hub", "GitHub.com");
        push(
            &mut conn,
            phone,
            &PushRequest {
                rules: vec![edited.clone()],
                ..Default::default()
            },
        )
        .expect("phone");
        let stale = rule("r1", 150, "get hub", "Github");
        assert_eq!(
            push(
                &mut conn,
                laptop,
                &PushRequest {
                    rules: vec![stale],
                    ..Default::default()
                }
            )
            .expect("stale")
            .rules,
            0
        );
        let changes = pull(&conn, first.cursor).expect("pull");
        assert_eq!(changes.rules, vec![edited.clone()]);

        // A deletion is a change too.
        edited.deleted = true;
        edited.updated_at = 300;
        push(
            &mut conn,
            laptop,
            &PushRequest {
                rules: vec![edited],
                ..Default::default()
            },
        )
        .expect("delete");
        let after = pull(&conn, changes.cursor).expect("pull");
        assert!(after.rules[0].deleted);
    }

    fn word(word: &str, at: i64, deleted: bool) -> WordRecord {
        WordRecord {
            word: word.into(),
            updated_at: at,
            deleted,
        }
    }

    #[test]
    fn words_newest_wins_and_share_the_rule_cursor() {
        let (mut conn, laptop, phone) = setup();
        let words = |w: Vec<WordRecord>| PushRequest {
            words: w,
            ..Default::default()
        };
        push(&mut conn, laptop, &words(vec![word("Hikaya", 100, false)])).expect("push");
        push(
            &mut conn,
            laptop,
            &PushRequest {
                rules: vec![rule("r1", 100, "get hub", "GitHub")],
                ..Default::default()
            },
        )
        .expect("rule");
        let first = pull(&conn, 0).expect("pull");
        assert_eq!(first.words, vec![word("Hikaya", 100, false)]);
        assert_eq!(first.rules.len(), 1);
        assert_eq!(first.cursor, 2);

        // The phone removes it; the laptop adding it again earlier loses.
        let removed = word("Hikaya", 300, true);
        assert_eq!(
            push(&mut conn, phone, &words(vec![removed.clone()]))
                .expect("phone")
                .words,
            Some(1)
        );
        assert_eq!(
            push(&mut conn, laptop, &words(vec![word("Hikaya", 200, false)]))
                .expect("stale")
                .words,
            Some(0)
        );
        let changes = pull(&conn, first.cursor).expect("pull");
        assert_eq!(changes.words, vec![removed]);
        assert!(changes.rules.is_empty());
    }

    #[test]
    fn snippets_newest_wins_and_flow_to_other_devices() {
        let (mut conn, laptop, phone) = setup();
        let snippet = |at: i64, text: &str, deleted: bool| SnippetRecord {
            id: "s1".into(),
            cue: "my address".into(),
            text: text.into(),
            updated_at: at,
            deleted,
        };
        let push_one = |conn: &mut Connection, device, s: SnippetRecord| {
            push(
                conn,
                device,
                &PushRequest {
                    snippets: vec![s],
                    ..Default::default()
                },
            )
            .expect("push")
            .snippets
        };
        assert_eq!(
            push_one(&mut conn, laptop, snippet(100, "12 rue\nParis", false)),
            Some(1)
        );
        let first = pull(&conn, 0).expect("pull");
        assert_eq!(first.snippets[0].text, "12 rue\nParis");
        assert_eq!(push_one(&mut conn, phone, snippet(300, "", true)), Some(1));
        assert_eq!(
            push_one(&mut conn, laptop, snippet(200, "old", false)),
            Some(0)
        );
        let changes = pull(&conn, first.cursor).expect("pull");
        assert_eq!(changes.snippets.len(), 1);
        assert!(changes.snippets[0].deleted);
        // A live snippet needs a cue and a text.
        let bad = push(
            &mut conn,
            laptop,
            &PushRequest {
                snippets: vec![SnippetRecord {
                    cue: "  ".into(),
                    ..snippet(400, "x", false)
                }],
                ..Default::default()
            },
        );
        assert!(matches!(bad, Err(PushError::Invalid(..))));
    }

    #[test]
    fn untrimmed_or_empty_words_are_refused() {
        let (mut conn, laptop, _) = setup();
        for bad in [" padded", "", "tab\there"] {
            let result = push(
                &mut conn,
                laptop,
                &PushRequest {
                    words: vec![word(bad, 1, false)],
                    ..Default::default()
                },
            );
            assert!(matches!(result, Err(PushError::Invalid(..))), "{bad:?}");
        }
    }

    #[test]
    fn substitutions_are_stored_once() {
        let (mut conn, laptop, _) = setup();
        let s = SubstitutionRecord {
            id: "s1".into(),
            from: "a med".into(),
            to: "Ahmed".into(),
            created_at: 5,
        };
        let request = PushRequest {
            substitutions: vec![s],
            ..Default::default()
        };
        assert_eq!(
            push(&mut conn, laptop, &request)
                .expect("push")
                .substitutions,
            1
        );
        assert_eq!(
            push(&mut conn, laptop, &request)
                .expect("again")
                .substitutions,
            0
        );
    }

    #[test]
    fn invalid_pushes_are_refused_whole() {
        let (mut conn, laptop, _) = setup();
        let too_many = PushRequest {
            dictations: (0..=MAX_RECORDS_PER_PUSH)
                .map(|i| dictation(&format!("d{i}"), 1, "x"))
                .collect(),
            ..Default::default()
        };
        assert!(matches!(
            push(&mut conn, laptop, &too_many),
            Err(PushError::TooLarge)
        ));
        for bad_id in ["", "has space", "../etc", &"x".repeat(65)] {
            let request = PushRequest {
                dictations: vec![dictation(bad_id, 1, "x")],
                ..Default::default()
            };
            assert!(
                matches!(
                    push(&mut conn, laptop, &request),
                    Err(PushError::Invalid(..))
                ),
                "{bad_id:?}"
            );
        }
        let mut backwards = dictation("d1", 10, "x");
        backwards.updated_at = 5;
        assert!(push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![backwards],
                ..Default::default()
            }
        )
        .is_err());
        let mut bad_source = rule("r1", 1, "a", "b");
        bad_source.source = "magic".into();
        assert!(push(
            &mut conn,
            laptop,
            &PushRequest {
                rules: vec![bad_source],
                ..Default::default()
            }
        )
        .is_err());
        assert!(texts(&conn, None).is_empty());
    }

    #[test]
    fn search_ignores_accents_and_case_and_treats_input_as_words() {
        let (mut conn, laptop, _) = setup();
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: vec![
                    dictation("d1", 1, "La réunion est décalée à jeudi"),
                    dictation("d2", 2, "Push the release on Friday"),
                    dictation("d3", 3, "Réunion d'équipe demain"),
                ],
                ..Default::default()
            },
        )
        .expect("push");
        assert_eq!(
            texts(&conn, Some("REUNION")),
            ["Réunion d'équipe demain", "La réunion est décalée à jeudi"]
        );
        assert_eq!(
            texts(&conn, Some("decal")),
            ["La réunion est décalée à jeudi"]
        );
        assert_eq!(
            texts(&conn, Some("release friday")),
            ["Push the release on Friday"]
        );
        // FTS syntax in the input is skipped: quotes, stars, brackets, column filters.
        assert_eq!(
            texts(&conn, Some("\"release\"* (^friday) -:")),
            ["Push the release on Friday"]
        );
        // Words like OR and NOT are searched for as words, not used as operators.
        assert!(texts(&conn, Some("release OR jeudi")).is_empty());
        assert_eq!(texts(&conn, Some("  ")).len(), 3);
    }

    #[test]
    fn listing_pages_backwards_in_time() {
        let (mut conn, laptop, _) = setup();
        let all: Vec<DictationRecord> = (1..=5)
            .map(|i| dictation(&format!("d{i}"), i, &format!("n{i}")))
            .collect();
        push(
            &mut conn,
            laptop,
            &PushRequest {
                dictations: all,
                ..Default::default()
            },
        )
        .expect("push");
        let page = dictations(&conn, None, None, 2).expect("page 1");
        assert_eq!(
            page.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(),
            ["n5", "n4"]
        );
        let next = dictations(&conn, None, Some(page[1].created_at), 2).expect("page 2");
        assert_eq!(
            next.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(),
            ["n3", "n2"]
        );
        assert_eq!(next[0].device, "laptop");
    }
}
