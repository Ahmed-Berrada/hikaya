//! Meetings and their transcript lines, in `hikaya.db`.

use crate::hikaya::store::{now_ms, LearningStore};
use anyhow::{Context, Result};
use hikaya_core::meeting::{in_order, remove_echo, Speaker, TranscriptLine};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use specta::Type;

/// Where a meeting is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    /// Audio is being captured.
    Recording,
    /// Recording stopped; the last segments are being transcribed.
    Transcribing,
    /// The transcript is complete.
    Done,
    /// The app stopped during the meeting. What was saved is kept.
    Interrupted,
}

impl MeetingStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Transcribing => "transcribing",
            Self::Done => "done",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "recording" => Self::Recording,
            "transcribing" => Self::Transcribing,
            "interrupted" => Self::Interrupted,
            _ => Self::Done,
        }
    }
}

/// A meeting as listed in the UI.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingSummary {
    pub id: i64,
    pub title: String,
    /// Unix ms.
    pub started_at: i64,
    pub duration_ms: i64,
    pub status: MeetingStatus,
    /// Whether the other people's audio was captured too.
    pub system_audio: bool,
    pub has_summary: bool,
    pub line_count: u32,
}

/// One transcript line as sent to the UI.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingLine {
    pub start_ms: i64,
    pub end_ms: i64,
    /// `you` or `others`.
    pub speaker: String,
    pub text: String,
}

/// A meeting with its whole transcript.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingDetail {
    pub meeting: MeetingSummary,
    pub lines: Vec<MeetingLine>,
    pub summary: Option<String>,
}

const SUMMARY_COLUMNS: &str =
    "m.id, m.title, m.started_at, m.duration_ms, m.status, m.system_audio,
    m.summary IS NOT NULL AND m.summary != '',
    (SELECT count(*) FROM meeting_lines l WHERE l.meeting_id = m.id)";

fn summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingSummary> {
    Ok(MeetingSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        started_at: row.get(2)?,
        duration_ms: row.get(3)?,
        status: MeetingStatus::parse(&row.get::<_, String>(4)?),
        system_audio: row.get(5)?,
        has_summary: row.get(6)?,
        line_count: row.get(7)?,
    })
}

impl LearningStore {
    /// Creates a meeting in the `recording` state and returns its id.
    pub fn create_meeting(&self, title: &str, started_at: i64, system_audio: bool) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO meetings (uid, title, started_at, system_audio, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4)",
            params![title.trim(), started_at, system_audio, now_ms()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Saves a meeting made from a phone recording, whole: its lines and the
    /// `done` state in one transaction, so a crash never leaves half of it.
    /// Saving the same recording again replaces the earlier transcript and
    /// keeps the meeting's id and sync id.
    pub fn save_phone_meeting(
        &self,
        recording: &str,
        title: &str,
        started_at: i64,
        duration_ms: i64,
        lines: &[TranscriptLine],
    ) -> Result<i64> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM meetings WHERE phone_recording = ?1",
                params![recording],
                |row| row.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                tx.execute("DELETE FROM meeting_lines WHERE meeting_id = ?1", [id])?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO meetings (uid, title, started_at, system_audio, updated_at,
                                           phone_recording)
                     VALUES (lower(hex(randomblob(16))), ?1, ?2, 0, ?3, ?4)",
                    params![title.trim(), started_at, now_ms(), recording],
                )?;
                tx.last_insert_rowid()
            }
        };
        for line in lines.iter().filter(|l| !l.text.trim().is_empty()) {
            tx.execute(
                "INSERT INTO meeting_lines (meeting_id, start_ms, end_ms, speaker, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    line.start_ms,
                    line.end_ms,
                    line.speaker.as_str(),
                    line.text.trim()
                ],
            )?;
        }
        tx.execute(
            "UPDATE meetings SET status = 'done', duration_ms = ?2, updated_at = ?3, synced = 0,
               phone_completed = 0
             WHERE id = ?1",
            params![id, duration_ms, now_ms()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Phone recordings whose meeting reached the hub, but the hub has not
    /// been told yet: `(recording id, meeting sync id)`.
    pub fn phone_completions(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let rows = conn
            .prepare(
                "SELECT phone_recording, uid FROM meetings
                 WHERE phone_recording IS NOT NULL AND phone_completed = 0 AND synced = 1",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Notes that the hub knows the recording became its meeting.
    pub fn mark_phone_completed(&self, recording: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE meetings SET phone_completed = 1 WHERE phone_recording = ?1",
            params![recording],
        )?;
        Ok(())
    }

    /// Saves one transcribed line.
    pub fn add_meeting_line(&self, meeting_id: i64, line: &TranscriptLine) -> Result<()> {
        if line.text.trim().is_empty() {
            return Ok(());
        }
        self.conn()?.execute(
            "INSERT INTO meeting_lines (meeting_id, start_ms, end_ms, speaker, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                meeting_id,
                line.start_ms,
                line.end_ms,
                line.speaker.as_str(),
                line.text.trim()
            ],
        )?;
        Ok(())
    }

    /// Updates the state and length of a meeting.
    pub fn set_meeting_status(
        &self,
        id: i64,
        status: MeetingStatus,
        duration_ms: i64,
    ) -> Result<()> {
        self.conn()?.execute(
            "UPDATE meetings SET status = ?2, duration_ms = max(duration_ms, ?3), updated_at = ?4, synced = 0
             WHERE id = ?1",
            params![id, status.as_str(), duration_ms, now_ms()],
        )?;
        Ok(())
    }

    /// Renames a meeting.
    pub fn rename_meeting(&self, id: i64, title: &str) -> Result<()> {
        let title = title.trim();
        anyhow::ensure!(!title.is_empty(), "a meeting needs a title");
        self.conn()?.execute(
            "UPDATE meetings SET title = ?2, updated_at = ?3, synced = 0 WHERE id = ?1",
            params![id, title, now_ms()],
        )?;
        Ok(())
    }

    /// Stores the summary of a meeting.
    pub fn set_meeting_summary(&self, id: i64, summary: &str, model: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE meetings SET summary = ?2, summary_model = ?3, updated_at = ?4, synced = 0 WHERE id = ?1",
            params![id, summary.trim(), model, now_ms()],
        )?;
        Ok(())
    }

    /// Meetings, newest first.
    pub fn list_meetings(&self) -> Result<Vec<MeetingSummary>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM meetings m ORDER BY m.started_at DESC"
        ))?;
        let meetings = statement
            .query_map([], summary_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(meetings)
    }

    /// The transcript lines, in order, with loudspeaker echo removed.
    pub fn meeting_lines(&self, id: i64) -> Result<Vec<TranscriptLine>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT start_ms, end_ms, speaker, text FROM meeting_lines WHERE meeting_id = ?1",
        )?;
        let lines = statement
            .query_map(params![id], |row| {
                Ok(TranscriptLine {
                    start_ms: row.get(0)?,
                    end_ms: row.get(1)?,
                    speaker: Speaker::parse(&row.get::<_, String>(2)?),
                    text: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(in_order(remove_echo(lines)))
    }

    /// A meeting with its transcript and summary.
    pub fn meeting_detail(&self, id: i64) -> Result<Option<MeetingDetail>> {
        let conn = self.conn()?;
        let found = conn
            .query_row(
                &format!("SELECT {SUMMARY_COLUMNS}, m.summary FROM meetings m WHERE m.id = ?1"),
                params![id],
                |row| Ok((summary_row(row)?, row.get::<_, Option<String>>(8)?)),
            )
            .optional()?;
        drop(conn);
        let Some((meeting, summary)) = found else {
            return Ok(None);
        };
        let lines = self
            .meeting_lines(id)?
            .into_iter()
            .map(|l| MeetingLine {
                start_ms: l.start_ms,
                end_ms: l.end_ms,
                speaker: l.speaker.as_str().to_string(),
                text: l.text,
            })
            .collect();
        Ok(Some(MeetingDetail {
            meeting,
            lines,
            summary,
        }))
    }

    /// Deletes a meeting and its transcript. The caller removes the audio.
    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])
            .context("deleting the meeting")?;
        Ok(())
    }

    /// Meetings left `recording` or `transcribing` by an app that stopped:
    /// marks them interrupted and returns their ids.
    pub fn mark_interrupted_meetings(&self) -> Result<Vec<i64>> {
        let conn = self.conn()?;
        let ids = conn
            .prepare("SELECT id FROM meetings WHERE status IN ('recording', 'transcribing')")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        conn.execute(
            "UPDATE meetings SET status = 'interrupted' WHERE status IN ('recording', 'transcribing')",
            [],
        )?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open");
        (dir, store)
    }

    fn line(start_ms: i64, speaker: Speaker, text: &str) -> TranscriptLine {
        TranscriptLine {
            start_ms,
            end_ms: start_ms + 2_000,
            speaker,
            text: text.into(),
        }
    }

    #[test]
    fn a_phone_meeting_is_saved_whole_once_and_completed_after_sync() {
        let (_dir, store) = store();
        let first = store
            .save_phone_meeting(
                "rec1",
                "Café avec Sam",
                1_000,
                60_000,
                &[
                    line(0, Speaker::Room, "Bonjour"),
                    line(30_000, Speaker::Room, " "),
                ],
            )
            .expect("save");
        let detail = store.meeting_detail(first).expect("detail").expect("found");
        assert_eq!(detail.meeting.status, MeetingStatus::Done);
        assert_eq!(detail.lines.len(), 1, "blank lines are dropped");
        assert_eq!(detail.lines[0].speaker, "room");

        // Transcribed again after a lapsed claim: same meeting, new lines.
        let again = store
            .save_phone_meeting(
                "rec1",
                "Café avec Sam",
                1_000,
                60_000,
                &[line(0, Speaker::Room, "Bonjour Sam")],
            )
            .expect("save again");
        assert_eq!(again, first);
        assert_eq!(
            store.meeting_lines(first).expect("lines")[0].text,
            "Bonjour Sam"
        );
        assert_eq!(store.list_meetings().expect("list").len(), 1);

        assert!(store
            .phone_completions()
            .expect("none before sync")
            .is_empty());
        store
            .conn()
            .expect("conn")
            .execute("UPDATE meetings SET synced = 1", [])
            .expect("synced");
        let pending = store.phone_completions().expect("after sync");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "rec1");
        store.mark_phone_completed("rec1").expect("mark");
        assert!(store.phone_completions().expect("done").is_empty());
    }

    #[test]
    fn a_meeting_keeps_its_lines_in_order_without_echo() {
        let (_dir, store) = store();
        let id = store
            .create_meeting(" Point produit ", 1_000, true)
            .expect("create");
        store
            .add_meeting_line(id, &line(30_000, Speaker::You, "Oui, jeudi me va."))
            .expect("add");
        store
            .add_meeting_line(id, &line(0, Speaker::Others, "On décale la démo à jeudi ?"))
            .expect("add");
        store
            .add_meeting_line(id, &line(400, Speaker::You, "on décale la démo à jeudi"))
            .expect("echo");
        store
            .add_meeting_line(id, &line(500, Speaker::You, "   "))
            .expect("blank is skipped");
        store
            .set_meeting_status(id, MeetingStatus::Done, 62_000)
            .expect("status");

        let detail = store.meeting_detail(id).expect("detail").expect("found");
        assert_eq!(detail.meeting.title, "Point produit");
        assert_eq!(detail.meeting.status, MeetingStatus::Done);
        assert_eq!(detail.meeting.duration_ms, 62_000);
        assert!(detail.meeting.system_audio);
        let texts: Vec<&str> = detail.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["On décale la démo à jeudi ?", "Oui, jeudi me va."]);
        assert_eq!(detail.lines[0].speaker, "others");
    }

    #[test]
    fn meetings_are_listed_newest_first_with_counts() {
        let (_dir, store) = store();
        let old = store.create_meeting("Lundi", 1_000, false).expect("create");
        let new = store.create_meeting("Mardi", 2_000, false).expect("create");
        store
            .add_meeting_line(new, &line(0, Speaker::You, "bonjour"))
            .expect("add");
        store
            .set_meeting_summary(new, "- Rien de neuf", "qwen3:4b-instruct")
            .expect("summary");
        let list = store.list_meetings().expect("list");
        assert_eq!(list.iter().map(|m| m.id).collect::<Vec<_>>(), [new, old]);
        assert_eq!(list[0].line_count, 1);
        assert!(list[0].has_summary);
        assert!(!list[1].has_summary);
    }

    #[test]
    fn deleting_a_meeting_removes_its_lines() {
        let (_dir, store) = store();
        let id = store.create_meeting("x", 1, false).expect("create");
        store
            .add_meeting_line(id, &line(0, Speaker::You, "secret"))
            .expect("add");
        store.delete_meeting(id).expect("delete");
        assert!(store.meeting_detail(id).expect("detail").is_none());
        let left: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM meeting_lines", [], |r| r.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }

    #[test]
    fn meetings_cut_off_by_a_crash_are_marked_interrupted() {
        let (_dir, store) = store();
        let a = store.create_meeting("a", 1, false).expect("a");
        let b = store.create_meeting("b", 2, false).expect("b");
        store
            .set_meeting_status(b, MeetingStatus::Done, 10)
            .expect("done");
        assert_eq!(store.mark_interrupted_meetings().expect("mark"), [a]);
        let list = store.list_meetings().expect("list");
        assert_eq!(
            list.iter().find(|m| m.id == a).expect("a").status,
            MeetingStatus::Interrupted
        );
        assert_eq!(
            list.iter().find(|m| m.id == b).expect("b").status,
            MeetingStatus::Done
        );
    }

    #[test]
    fn rename_refuses_an_empty_title() {
        let (_dir, store) = store();
        let id = store.create_meeting("x", 1, false).expect("create");
        assert!(store.rename_meeting(id, "  ").is_err());
        store.rename_meeting(id, "Revue").expect("rename");
        assert_eq!(store.list_meetings().expect("list")[0].title, "Revue");
    }
}
