//! Meetings pushed by devices: storage, listing and search.

use crate::sync::PushError;
use anyhow::{Context, Result};
use hikaya_core::sync::{MeetingLineRecord, MeetingRecord, MAX_MEETING_LINES, MAX_TEXT_BYTES};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;

/// Checks a meeting before anything is written.
pub fn validate(meeting: &MeetingRecord) -> Result<(), PushError> {
    let invalid = |why: &str| Err(PushError::Invalid(meeting.id.clone(), why.into()));
    if meeting.title.trim().is_empty() || meeting.title.len() > 200 {
        return invalid("a meeting needs a title of at most 200 bytes");
    }
    if meeting.lines.len() > MAX_MEETING_LINES {
        return invalid("too many transcript lines");
    }
    if meeting
        .summary
        .as_ref()
        .is_some_and(|s| s.len() > MAX_TEXT_BYTES)
    {
        return invalid("the summary is too long");
    }
    for line in &meeting.lines {
        if !matches!(line.speaker.as_str(), "you" | "others" | "room") {
            return invalid("a line's speaker must be you, others or room");
        }
        if line.text.len() > MAX_TEXT_BYTES || line.end_ms < line.start_ms {
            return invalid("a transcript line is invalid");
        }
    }
    Ok(())
}

/// Stores a meeting, or replaces it when this version is newer. Only the
/// device that recorded a meeting may change it. Returns whether anything
/// changed.
pub fn apply(
    tx: &Transaction<'_>,
    device: i64,
    meeting: &MeetingRecord,
) -> Result<bool, PushError> {
    let current: Option<(i64, i64)> = tx
        .query_row(
            "SELECT device_id, updated_at FROM meetings WHERE id = ?1",
            params![meeting.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("reading a meeting")?;
    match current {
        Some((owner, _)) if owner != device => return Err(PushError::NotYours(meeting.id.clone())),
        Some((_, updated_at)) if updated_at >= meeting.updated_at => return Ok(false),
        _ => {}
    }
    let lines_json = serde_json::to_string(&meeting.lines).context("encoding the transcript")?;
    let transcript = meeting
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    tx.execute(
        "INSERT INTO meetings (id, device_id, title, started_at, duration_ms, updated_at, summary,
                               lines_json, transcript, line_count, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, strftime('%s','now') * 1000)
         ON CONFLICT (id) DO UPDATE SET title = ?3, started_at = ?4, duration_ms = ?5,
           updated_at = ?6, summary = ?7, lines_json = ?8, transcript = ?9, line_count = ?10",
        params![
            meeting.id,
            device,
            meeting.title.trim(),
            meeting.started_at,
            meeting.duration_ms,
            meeting.updated_at,
            meeting.summary,
            lines_json,
            transcript,
            meeting.lines.len() as i64
        ],
    )
    .context("saving a meeting")?;
    Ok(true)
}

/// A meeting as listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MeetingView {
    pub id: String,
    pub device: String,
    pub title: String,
    pub started_at: i64,
    pub duration_ms: i64,
    pub has_summary: bool,
    pub line_count: i64,
}

/// A meeting with its transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MeetingDetailView {
    #[serde(flatten)]
    pub meeting: MeetingView,
    pub summary: Option<String>,
    pub lines: Vec<MeetingLineRecord>,
}

const COLUMNS: &str = "m.id, v.name, m.title, m.started_at, m.duration_ms,
    m.summary IS NOT NULL AND m.summary != '', m.line_count";

fn view(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingView> {
    Ok(MeetingView {
        id: row.get(0)?,
        device: row.get(1)?,
        title: row.get(2)?,
        started_at: row.get(3)?,
        duration_ms: row.get(4)?,
        has_summary: row.get(5)?,
        line_count: row.get(6)?,
    })
}

/// Newest meetings first, optionally only those matching `query` in their
/// title, summary or transcript. The query is reduced to words, as for
/// dictations.
pub fn list(
    conn: &Connection,
    query: Option<&str>,
    before: Option<i64>,
    limit: usize,
) -> Result<Vec<MeetingView>> {
    let before = before.unwrap_or(i64::MAX);
    let limit = limit.clamp(1, 200) as i64;
    let words = crate::sync::fts_words(query.unwrap_or(""));
    let rows = if words.is_empty() {
        conn.prepare(&format!(
            "SELECT {COLUMNS} FROM meetings m JOIN devices v ON v.id = m.device_id
             WHERE m.started_at < ?1 ORDER BY m.started_at DESC LIMIT ?2"
        ))?
        .query_map(params![before, limit], view)?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        conn.prepare(&format!(
            "SELECT {COLUMNS} FROM meetings_fts f JOIN meetings m ON m.rowid = f.rowid
             JOIN devices v ON v.id = m.device_id
             WHERE meetings_fts MATCH ?1 AND m.started_at < ?2
             ORDER BY m.started_at DESC LIMIT ?3"
        ))?
        .query_map(params![words, before, limit], view)?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

/// One meeting with its transcript.
pub fn detail(conn: &Connection, id: &str) -> Result<Option<MeetingDetailView>> {
    let found = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS}, m.summary, m.lines_json FROM meetings m
                 JOIN devices v ON v.id = m.device_id WHERE m.id = ?1"
            ),
            params![id],
            |row| {
                Ok((
                    view(row)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((meeting, summary, lines_json)) = found else {
        return Ok(None);
    };
    Ok(Some(MeetingDetailView {
        meeting,
        summary,
        lines: serde_json::from_str(&lines_json).context("reading a stored transcript")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, devices, sync};
    use hikaya_core::sync::PushRequest;

    fn setup() -> (Connection, i64, i64) {
        let mut conn = Connection::open_in_memory().expect("open");
        db::migrate(&mut conn).expect("migrate");
        let (laptop, _) = devices::register(&conn, "laptop").expect("laptop");
        let (phone, _) = devices::register(&conn, "phone").expect("phone");
        (conn, laptop.id, phone.id)
    }

    fn meeting(id: &str, updated_at: i64, title: &str) -> MeetingRecord {
        MeetingRecord {
            id: id.into(),
            title: title.into(),
            started_at: 1_000,
            duration_ms: 60_000,
            updated_at,
            summary: None,
            lines: vec![
                MeetingLineRecord {
                    start_ms: 0,
                    end_ms: 30_000,
                    speaker: "others".into(),
                    text: "On décale la démo à jeudi ?".into(),
                },
                MeetingLineRecord {
                    start_ms: 30_000,
                    end_ms: 60_000,
                    speaker: "you".into(),
                    text: "Oui, jeudi 14h.".into(),
                },
            ],
        }
    }

    fn push(
        conn: &mut Connection,
        device: i64,
        meetings: Vec<MeetingRecord>,
    ) -> Result<usize, PushError> {
        sync::push(
            conn,
            device,
            &PushRequest {
                meetings,
                ..Default::default()
            },
        )
        .map(|r| r.meetings)
    }

    #[test]
    fn a_meeting_is_stored_once_and_replaced_by_a_newer_version() {
        let (mut conn, laptop, _) = setup();
        assert_eq!(
            push(&mut conn, laptop, vec![meeting("m1", 10, "Point")]).expect("push"),
            1
        );
        assert_eq!(
            push(&mut conn, laptop, vec![meeting("m1", 10, "Point")]).expect("again"),
            0
        );
        let mut renamed = meeting("m1", 20, "Point produit");
        renamed.summary = Some("## TL;DR\\nDémo jeudi.".into());
        assert_eq!(push(&mut conn, laptop, vec![renamed]).expect("newer"), 1);
        assert_eq!(
            push(&mut conn, laptop, vec![meeting("m1", 15, "stale")]).expect("older"),
            0
        );

        let found = detail(&conn, "m1").expect("detail").expect("found");
        assert_eq!(found.meeting.title, "Point produit");
        assert!(found.meeting.has_summary);
        assert_eq!(found.meeting.device, "laptop");
        assert_eq!(found.lines.len(), 2);
        assert_eq!(found.lines[1].speaker, "you");
    }

    #[test]
    fn meetings_are_searchable_by_title_summary_and_transcript() {
        let (mut conn, laptop, _) = setup();
        let mut a = meeting("m1", 1, "Revue produit");
        a.summary = Some("Budget validé".into());
        let mut b = meeting("m2", 1, "Weekly");
        b.started_at = 2_000;
        b.lines[0].text = "Let's review the hiring plan".into();
        push(&mut conn, laptop, vec![a, b]).expect("push");
        let titles = |q: &str| -> Vec<String> {
            list(&conn, Some(q), None, 10)
                .expect("list")
                .into_iter()
                .map(|m| m.title)
                .collect()
        };
        assert_eq!(titles("revue"), ["Revue produit"]);
        assert_eq!(titles("budget valide"), ["Revue produit"]);
        assert_eq!(titles("hiring"), ["Weekly"]);
        assert_eq!(titles("decale"), ["Revue produit"]);
        assert_eq!(titles("jeudi"), ["Weekly", "Revue produit"]);
        assert_eq!(list(&conn, None, None, 10).expect("all")[0].title, "Weekly");
    }

    #[test]
    fn another_device_cannot_overwrite_a_meeting() {
        let (mut conn, laptop, phone) = setup();
        push(&mut conn, laptop, vec![meeting("m1", 1, "Mine")]).expect("push");
        assert!(matches!(
            push(&mut conn, phone, vec![meeting("m1", 9, "Theirs")]),
            Err(PushError::NotYours(_))
        ));
        assert_eq!(
            detail(&conn, "m1").expect("d").expect("f").meeting.title,
            "Mine"
        );
    }

    #[test]
    fn invalid_meetings_are_refused_whole() {
        let (mut conn, laptop, _) = setup();
        let mut bad_speaker = meeting("m1", 1, "x");
        bad_speaker.lines[0].speaker = "<script>".into();
        let mut no_title = meeting("m2", 1, "   ");
        no_title.lines.clear();
        let mut backwards = meeting("m3", 1, "x");
        backwards.lines[0].end_ms = -1;
        for m in [bad_speaker, no_title, backwards] {
            assert!(matches!(
                push(&mut conn, laptop, vec![m]),
                Err(PushError::Invalid(..))
            ));
        }
        assert!(list(&conn, None, None, 10).expect("list").is_empty());
    }
}
