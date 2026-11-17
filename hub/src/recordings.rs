//! Recordings made on a phone, waiting for a computer to transcribe them.
//! See `docs/adr/0004-phone-recordings.md`.
//!
//! A recording's life:
//!
//! 1. `recording`: the phone creates it and uploads numbered chunks.
//! 2. `pending`: the phone says it is finished and every chunk is there.
//! 3. `claimed`: a computer took it and is transcribing. A claim older than
//!    [`CLAIM_TIMEOUT_MS`] lapses, so a computer that went to sleep does not
//!    hold it forever.
//! 4. `done`: the computer sent the meeting; the audio is deleted.
//!
//! A computer that fails gives the recording back; after [`MAX_ATTEMPTS`]
//! failures it stays `failed`, with its audio, until someone retries it.

use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Largest chunk accepted. The phone sends a few seconds at a time.
pub const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// Largest recording: about eight hours of phone AAC.
pub const MAX_RECORDING_BYTES: i64 = 256 * 1024 * 1024;
/// How long a computer may hold a recording before another may take it.
pub const CLAIM_TIMEOUT_MS: i64 = 2 * 60 * 60 * 1000;
/// Failed transcriptions before a recording stops being handed out.
pub const MAX_ATTEMPTS: i64 = 3;
/// Audio types a phone may send.
const MIME_TYPES: &[&str] = &[
    "audio/mp4",
    "audio/aac",
    "audio/webm",
    "audio/ogg",
    "audio/wav",
];

/// Why a recording request was refused.
#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("no such recording")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error("the recording is too large")]
    TooLarge,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<rusqlite::Error> for RecordingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Other(error.into())
    }
}

/// A recording as listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordingView {
    pub id: String,
    pub device: String,
    pub title: String,
    pub mime: String,
    pub created_at: i64,
    pub status: String,
    pub chunks: i64,
    pub bytes: i64,
    pub attempts: i64,
    pub error: Option<String>,
    /// The meeting made from it, once done.
    pub meeting_id: Option<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const COLUMNS: &str = "r.id, v.name, r.title, r.mime, r.created_at, r.status,
    (SELECT count(*) FROM recording_chunks c WHERE c.recording_id = r.id), r.bytes,
    r.attempts, r.error, r.meeting_id";

fn view(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingView> {
    Ok(RecordingView {
        id: row.get(0)?,
        device: row.get(1)?,
        title: row.get(2)?,
        mime: row.get(3)?,
        created_at: row.get(4)?,
        status: row.get(5)?,
        chunks: row.get(6)?,
        bytes: row.get(7)?,
        attempts: row.get(8)?,
        error: row.get(9)?,
        meeting_id: row.get(10)?,
    })
}

/// One recording.
pub fn get(conn: &Connection, id: &str) -> Result<Option<RecordingView>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM recordings r JOIN devices v ON v.id = r.device_id
                 WHERE r.id = ?1"
            ),
            params![id],
            view,
        )
        .optional()?)
}

fn require(conn: &Connection, id: &str) -> Result<RecordingView, RecordingError> {
    get(conn, id)?.ok_or(RecordingError::NotFound)
}

/// Recordings not yet turned into meetings, newest first, plus the latest
/// finished ones so the phone can show what became of them.
pub fn list(conn: &Connection, limit: usize) -> Result<Vec<RecordingView>> {
    let limit = limit.clamp(1, 200) as i64;
    let rows = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM recordings r JOIN devices v ON v.id = r.device_id
             ORDER BY r.created_at DESC LIMIT ?1"
        ))?
        .query_map(params![limit], view)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Starts a recording. The base type of `mime` (without codecs) is kept.
pub fn create(
    conn: &Connection,
    device: i64,
    title: &str,
    mime: &str,
) -> Result<RecordingView, RecordingError> {
    let title = title.trim();
    if title.is_empty() || title.len() > 200 {
        return Err(RecordingError::Invalid(
            "a recording needs a title of at most 200 bytes".into(),
        ));
    }
    let mime = mime.split(';').next().unwrap_or("").trim().to_lowercase();
    if !MIME_TYPES.contains(&mime.as_str()) {
        return Err(RecordingError::Invalid(format!(
            "unsupported audio type '{mime}'"
        )));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO recordings (id, device_id, title, mime, created_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, 'recording')",
        params![id, device, title, mime, now_ms()],
    )
    .context("creating a recording")?;
    require(conn, &id)
}

/// Stores chunk `index`. Sending the same chunk again replaces it, so a
/// phone can retry an upload it is unsure about.
pub fn put_chunk(
    conn: &mut Connection,
    device: i64,
    id: &str,
    index: i64,
    data: &[u8],
) -> Result<(), RecordingError> {
    if data.is_empty() || data.len() > MAX_CHUNK_BYTES {
        return Err(RecordingError::Invalid(format!(
            "a chunk must hold 1 to {MAX_CHUNK_BYTES} bytes"
        )));
    }
    if !(0..100_000).contains(&index) {
        return Err(RecordingError::Invalid("chunk index out of range".into()));
    }
    let tx = conn.transaction().context("starting a transaction")?;
    let found: Option<(i64, String)> = tx
        .query_row(
            "SELECT device_id, status FROM recordings WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match found {
        None => return Err(RecordingError::NotFound),
        Some((owner, _)) if owner != device => {
            return Err(RecordingError::Conflict(
                "only the device that started a recording can add to it".into(),
            ))
        }
        Some((_, status)) if status != "recording" => {
            return Err(RecordingError::Conflict(
                "the recording is already finished".into(),
            ))
        }
        _ => {}
    }
    let previous: i64 = tx
        .query_row(
            "SELECT length(data) FROM recording_chunks WHERE recording_id = ?1 AND idx = ?2",
            params![id, index],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let total: i64 = tx.query_row(
        "SELECT bytes FROM recordings WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let total = total - previous + data.len() as i64;
    if total > MAX_RECORDING_BYTES {
        return Err(RecordingError::TooLarge);
    }
    tx.execute(
        "INSERT INTO recording_chunks (recording_id, idx, data) VALUES (?1, ?2, ?3)
         ON CONFLICT (recording_id, idx) DO UPDATE SET data = ?3",
        params![id, index, data],
    )?;
    tx.execute(
        "UPDATE recordings SET bytes = ?2 WHERE id = ?1",
        params![id, total],
    )?;
    tx.commit().context("saving a chunk")?;
    Ok(())
}

/// Marks a recording as complete with `chunks` chunks, numbered from 0. Any
/// missing chunk is refused, naming the first one, so the phone can resend it.
pub fn finish(
    conn: &Connection,
    device: i64,
    id: &str,
    chunks: i64,
) -> Result<RecordingView, RecordingError> {
    let current = require(conn, id)?;
    let owner: i64 = conn.query_row(
        "SELECT device_id FROM recordings WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    if owner != device {
        return Err(RecordingError::Conflict(
            "only the device that started a recording can finish it".into(),
        ));
    }
    if current.status != "recording" {
        // Finishing twice is fine: the phone may not have seen the answer.
        return Ok(current);
    }
    if chunks < 1 || current.chunks != chunks {
        let first_missing: i64 = conn.query_row(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?2)
             SELECT coalesce(min(i), -1) FROM n
             WHERE i NOT IN (SELECT idx FROM recording_chunks WHERE recording_id = ?1)",
            params![id, chunks.max(1)],
            |row| row.get(0),
        )?;
        return Err(RecordingError::Conflict(if first_missing >= 0 {
            format!("chunk {first_missing} is missing")
        } else {
            format!("expected {chunks} chunks, have {}", current.chunks)
        }));
    }
    conn.execute(
        "UPDATE recordings SET status = 'pending' WHERE id = ?1",
        params![id],
    )?;
    require(conn, id)
}

/// Hands the oldest waiting recording to `device`: pending ones, and claimed
/// ones whose claim lapsed. `None` when there is nothing to do.
pub fn claim(conn: &Connection, device: i64) -> Result<Option<RecordingView>> {
    let now = now_ms();
    let id: Option<String> = conn
        .query_row(
            "UPDATE recordings SET status = 'claimed', claimed_by = ?1, claimed_at = ?2
             WHERE id = (
               SELECT id FROM recordings
               WHERE (status = 'pending' AND attempts < ?3)
                  OR (status = 'claimed' AND claimed_at < ?4)
               ORDER BY created_at LIMIT 1)
             RETURNING id",
            params![device, now, MAX_ATTEMPTS, now - CLAIM_TIMEOUT_MS],
            |row| row.get(0),
        )
        .optional()?;
    match id {
        Some(id) => get(conn, &id),
        None => Ok(None),
    }
}

fn require_claim(conn: &Connection, device: i64, id: &str) -> Result<(), RecordingError> {
    let found: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT status, claimed_by FROM recordings WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match found {
        None => Err(RecordingError::NotFound),
        Some((status, Some(by))) if status == "claimed" && by == device => Ok(()),
        Some(_) => Err(RecordingError::Conflict(
            "this device does not hold the recording; claim it first".into(),
        )),
    }
}

/// The audio, chunks joined in order. Only for the device holding the claim.
pub fn audio(
    conn: &Connection,
    device: i64,
    id: &str,
) -> Result<(String, Vec<u8>), RecordingError> {
    require_claim(conn, device, id)?;
    let mime: String = conn.query_row(
        "SELECT mime FROM recordings WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let mut statement =
        conn.prepare("SELECT data FROM recording_chunks WHERE recording_id = ?1 ORDER BY idx")?;
    let mut audio = Vec::new();
    let mut rows = statement.query(params![id])?;
    while let Some(row) = rows.next()? {
        let chunk: Vec<u8> = row.get(0)?;
        audio.extend_from_slice(&chunk);
    }
    Ok((mime, audio))
}

/// Records that the recording became `meeting_id` and deletes its audio.
pub fn complete(
    conn: &mut Connection,
    device: i64,
    id: &str,
    meeting_id: &str,
) -> Result<RecordingView, RecordingError> {
    require_claim(conn, device, id)?;
    let exists: bool = conn.query_row(
        "SELECT count(*) > 0 FROM meetings WHERE id = ?1 AND device_id = ?2",
        params![meeting_id, device],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(RecordingError::Conflict(
            "push the meeting before completing the recording".into(),
        ));
    }
    let tx = conn.transaction().context("starting a transaction")?;
    tx.execute(
        "DELETE FROM recording_chunks WHERE recording_id = ?1",
        params![id],
    )?;
    tx.execute(
        "UPDATE recordings SET status = 'done', meeting_id = ?2, error = NULL WHERE id = ?1",
        params![id, meeting_id],
    )?;
    tx.commit().context("completing a recording")?;
    require(conn, id)
}

/// Gives a recording back after a failed transcription. It is handed out
/// again until it has failed [`MAX_ATTEMPTS`] times.
pub fn fail(
    conn: &Connection,
    device: i64,
    id: &str,
    error: &str,
) -> Result<RecordingView, RecordingError> {
    require_claim(conn, device, id)?;
    let error: String = error.chars().take(500).collect();
    conn.execute(
        "UPDATE recordings SET attempts = attempts + 1, error = ?2, claimed_by = NULL,
           claimed_at = NULL,
           status = CASE WHEN attempts + 1 >= ?3 THEN 'failed' ELSE 'pending' END
         WHERE id = ?1",
        params![id, error, MAX_ATTEMPTS],
    )?;
    require(conn, id)
}

/// Makes a failed recording available again.
pub fn retry(conn: &Connection, id: &str) -> Result<RecordingView, RecordingError> {
    let current = require(conn, id)?;
    if current.status != "failed" {
        return Err(RecordingError::Conflict(
            "only a failed recording can be retried".into(),
        ));
    }
    conn.execute(
        "UPDATE recordings SET status = 'pending', attempts = 0 WHERE id = ?1",
        params![id],
    )?;
    require(conn, id)
}

/// Deletes a recording and its audio. The meeting made from it stays.
pub fn delete(conn: &mut Connection, id: &str) -> Result<(), RecordingError> {
    let tx = conn.transaction().context("starting a transaction")?;
    tx.execute(
        "DELETE FROM recording_chunks WHERE recording_id = ?1",
        params![id],
    )?;
    let removed = tx.execute("DELETE FROM recordings WHERE id = ?1", params![id])?;
    tx.commit().context("deleting a recording")?;
    if removed == 0 {
        return Err(RecordingError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, devices};

    fn setup() -> (Connection, i64, i64) {
        let mut conn = Connection::open_in_memory().expect("open");
        db::migrate(&mut conn).expect("migrate");
        let (phone, _) = devices::register(&conn, "phone").expect("phone");
        let (laptop, _) = devices::register(&conn, "laptop").expect("laptop");
        (conn, phone.id, laptop.id)
    }

    fn recorded(conn: &mut Connection, phone: i64) -> String {
        let r = create(conn, phone, "Café avec Sam", "audio/mp4;codecs=mp4a.40.2").expect("create");
        assert_eq!(r.mime, "audio/mp4");
        assert_eq!(r.status, "recording");
        put_chunk(conn, phone, &r.id, 0, b"abc").expect("chunk 0");
        put_chunk(conn, phone, &r.id, 1, b"def").expect("chunk 1");
        finish(conn, phone, &r.id, 2).expect("finish");
        r.id
    }

    fn add_meeting(conn: &Connection, device: i64, id: &str) {
        conn.execute(
            "INSERT INTO meetings (id, device_id, title, started_at, duration_ms, updated_at,
               lines_json, transcript, line_count, received_at)
             VALUES (?1, ?2, 't', 0, 0, 0, '[]', '', 0, 0)",
            params![id, device],
        )
        .expect("meeting");
    }

    #[test]
    fn a_recording_goes_from_the_phone_to_a_computer_and_loses_its_audio() {
        let (mut conn, phone, laptop) = setup();
        let id = recorded(&mut conn, phone);
        assert_eq!(get(&conn, &id).expect("get").expect("found").bytes, 6);

        let claimed = claim(&conn, laptop).expect("claim").expect("one waiting");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.status, "claimed");
        assert!(claim(&conn, laptop).expect("claim").is_none());

        let (mime, audio_bytes) = audio(&conn, laptop, &id).expect("audio");
        assert_eq!(mime, "audio/mp4");
        assert_eq!(audio_bytes, b"abcdef");
        assert!(matches!(
            audio(&conn, phone, &id),
            Err(RecordingError::Conflict(_))
        ));

        assert!(matches!(
            complete(&mut conn, laptop, &id, "m1"),
            Err(RecordingError::Conflict(_))
        ));
        add_meeting(&conn, laptop, "m1");
        let done = complete(&mut conn, laptop, &id, "m1").expect("complete");
        assert_eq!(done.status, "done");
        assert_eq!(done.meeting_id.as_deref(), Some("m1"));
        assert_eq!(done.chunks, 0);
    }

    #[test]
    fn chunks_can_be_resent_and_a_gap_is_named() {
        let (mut conn, phone, _) = setup();
        let r = create(&conn, phone, "x", "audio/webm;codecs=opus").expect("create");
        put_chunk(&mut conn, phone, &r.id, 0, b"aaaa").expect("0");
        put_chunk(&mut conn, phone, &r.id, 0, b"aa").expect("0 again");
        put_chunk(&mut conn, phone, &r.id, 2, b"cc").expect("2");
        assert_eq!(get(&conn, &r.id).expect("g").expect("f").bytes, 4);
        match finish(&conn, phone, &r.id, 3) {
            Err(RecordingError::Conflict(why)) => assert_eq!(why, "chunk 1 is missing"),
            other => panic!("expected a conflict, got {other:?}"),
        }
        put_chunk(&mut conn, phone, &r.id, 1, b"bb").expect("1");
        assert_eq!(
            finish(&conn, phone, &r.id, 3).expect("finish").status,
            "pending"
        );
        assert_eq!(
            finish(&conn, phone, &r.id, 3).expect("again").status,
            "pending"
        );
        assert!(matches!(
            put_chunk(&mut conn, phone, &r.id, 3, b"late"),
            Err(RecordingError::Conflict(_))
        ));
    }

    #[test]
    fn only_the_recording_device_can_add_to_it() {
        let (mut conn, phone, laptop) = setup();
        let r = create(&conn, phone, "x", "audio/mp4").expect("create");
        assert!(matches!(
            put_chunk(&mut conn, laptop, &r.id, 0, b"x"),
            Err(RecordingError::Conflict(_))
        ));
        assert!(matches!(
            put_chunk(&mut conn, phone, "nope", 0, b"x"),
            Err(RecordingError::NotFound)
        ));
    }

    #[test]
    fn bad_input_is_refused() {
        let (mut conn, phone, _) = setup();
        assert!(matches!(
            create(&conn, phone, "  ", "audio/mp4"),
            Err(RecordingError::Invalid(_))
        ));
        assert!(matches!(
            create(&conn, phone, "x", "text/html"),
            Err(RecordingError::Invalid(_))
        ));
        let r = create(&conn, phone, "x", "audio/mp4").expect("create");
        assert!(matches!(
            put_chunk(&mut conn, phone, &r.id, 0, b""),
            Err(RecordingError::Invalid(_))
        ));
        assert!(matches!(
            put_chunk(&mut conn, phone, &r.id, -1, b"x"),
            Err(RecordingError::Invalid(_))
        ));
        assert!(matches!(
            finish(&conn, phone, &r.id, 0),
            Err(RecordingError::Conflict(_))
        ));
    }

    #[test]
    fn a_failed_recording_is_retried_then_parked() {
        let (mut conn, phone, laptop) = setup();
        let id = recorded(&mut conn, phone);
        for attempt in 1..=MAX_ATTEMPTS {
            claim(&conn, laptop).expect("claim").expect("waiting");
            let r = fail(&conn, laptop, &id, "decoder error").expect("fail");
            assert_eq!(r.attempts, attempt);
        }
        let parked = get(&conn, &id).expect("g").expect("f");
        assert_eq!(parked.status, "failed");
        assert_eq!(parked.error.as_deref(), Some("decoder error"));
        assert_eq!(parked.bytes, 6, "the audio is kept");
        assert!(claim(&conn, laptop).expect("claim").is_none());
        assert_eq!(retry(&conn, &id).expect("retry").status, "pending");
        assert!(claim(&conn, laptop).expect("claim").is_some());
    }

    #[test]
    fn a_lapsed_claim_can_be_taken_by_another_computer() {
        let (mut conn, phone, laptop) = setup();
        let (desktop, _) = devices::register(&conn, "desktop").expect("desktop");
        let id = recorded(&mut conn, phone);
        claim(&conn, laptop).expect("claim").expect("waiting");
        assert!(claim(&conn, desktop.id).expect("claim").is_none());
        conn.execute(
            "UPDATE recordings SET claimed_at = claimed_at - ?1",
            params![CLAIM_TIMEOUT_MS + 1],
        )
        .expect("age the claim");
        assert_eq!(
            claim(&conn, desktop.id).expect("claim").expect("lapsed").id,
            id
        );
        assert!(matches!(
            audio(&conn, laptop, &id),
            Err(RecordingError::Conflict(_))
        ));
    }

    #[test]
    fn deleting_removes_the_audio() {
        let (mut conn, phone, _) = setup();
        let id = recorded(&mut conn, phone);
        delete(&mut conn, &id).expect("delete");
        let left: i64 = conn
            .query_row("SELECT count(*) FROM recording_chunks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(left, 0);
        assert!(matches!(
            delete(&mut conn, &id),
            Err(RecordingError::NotFound)
        ));
        assert_eq!(list(&conn, 10).expect("list").len(), 0);
    }
}
