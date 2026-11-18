//! Summaries made on the hub, as a fallback.
//!
//! Computers summarize their meetings with their own AI provider. A meeting
//! that reaches the hub without a summary (no provider was set up, or the
//! user never asked) is summarized here with a local Ollama model, once it
//! has waited long enough for its computer to do it.
//!
//! The result goes in `hub_summary`, never in `summary`: the meeting still
//! belongs to the device that recorded it, and a summary that device sends
//! later takes precedence.

use anyhow::{anyhow, Context, Result};
use hikaya_core::meeting::{
    merge_prompt, strip_think, summary_prompts, Speaker, SpeakerNames, TranscriptLine,
};
use hikaya_core::sync::MeetingLineRecord;
use rusqlite::{params, Connection, OptionalExtension};
use std::time::Duration;

/// How often the hub tries the same meeting again after a failure.
const RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// What answers the summary questions. A trait so tests need no model.
pub trait Llm {
    fn ask(&self, prompt: &str) -> Result<String>;
}

/// A model served by Ollama.
pub struct Ollama {
    url: String,
    model: String,
    agent: ureq::Agent,
}

#[derive(serde::Deserialize)]
struct ChatAnswer {
    message: ChatMessage,
}

#[derive(serde::Deserialize)]
struct ChatMessage {
    content: String,
}

impl Ollama {
    /// `url` is Ollama's base address, such as `http://ollama:11434`.
    pub fn new(url: &str, model: &str) -> Self {
        // A 4B model on a laptop CPU writes about 8 tokens per second.
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20 * 60)))
            .build()
            .into();
        Self {
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            agent,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl Llm for Ollama {
    fn ask(&self, prompt: &str) -> Result<String> {
        let mut response = self
            .agent
            .post(format!("{}/api/chat", self.url))
            .send_json(serde_json::json!({
                "model": self.model,
                "messages": [{ "role": "user", "content": prompt }],
                "stream": false,
                // Room for a 12,000-character part plus the instructions.
                "options": { "num_ctx": 8192, "temperature": 0.2 },
            }))
            .context("asking Ollama")?;
        let answer: ChatAnswer = response
            .body_mut()
            .read_json()
            .context("reading Ollama's answer")?;
        let text = strip_think(&answer.message.content).trim().to_string();
        if text.is_empty() {
            return Err(anyhow!("the model returned an empty summary"));
        }
        Ok(text)
    }
}

/// Summarizes a transcript: each part, then the parts together.
pub fn summarize(llm: &dyn Llm, lines: &[TranscriptLine]) -> Result<String> {
    let names = SpeakerNames {
        you: "You".into(),
        others: "Others".into(),
        room: "Room".into(),
    };
    let mut parts = Vec::new();
    for prompt in summary_prompts(lines, &names) {
        parts.push(llm.ask(&prompt)?);
    }
    match parts.len() {
        0 => Err(anyhow!("the meeting has no transcript")),
        1 => Ok(parts.remove(0)),
        _ => llm.ask(&merge_prompt(lines, &parts)),
    }
}

/// The oldest meeting that needs a summary from the hub: none from its
/// computer, received at least `delay_ms` ago, and not tried in a day.
/// Marks it as tried, so a crash or failure does not retry it at once.
pub fn take_next(
    conn: &Connection,
    now_ms: i64,
    delay_ms: i64,
) -> Result<Option<(String, Vec<TranscriptLine>)>> {
    let found: Option<(String, String)> = conn
        .query_row(
            "UPDATE meetings SET hub_summary_tried_at = ?1
             WHERE id = (
               SELECT id FROM meetings
               WHERE (summary IS NULL OR summary = '') AND hub_summary IS NULL
                 AND line_count > 0 AND received_at <= ?2
                 AND (hub_summary_tried_at IS NULL OR hub_summary_tried_at <= ?3)
               ORDER BY received_at LIMIT 1)
             RETURNING id, lines_json",
            params![now_ms, now_ms - delay_ms, now_ms - RETRY_AFTER_MS],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("choosing a meeting to summarize")?;
    let Some((id, lines_json)) = found else {
        return Ok(None);
    };
    let records: Vec<MeetingLineRecord> =
        serde_json::from_str(&lines_json).context("reading a stored transcript")?;
    let lines = records
        .into_iter()
        .map(|l| TranscriptLine {
            start_ms: l.start_ms,
            end_ms: l.end_ms,
            speaker: Speaker::parse(&l.speaker),
            text: l.text,
        })
        .collect();
    Ok(Some((id, lines)))
}

/// Stores a summary made by the hub.
pub fn save(conn: &Connection, id: &str, summary: &str, model: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET hub_summary = ?2, hub_summary_model = ?3 WHERE id = ?1",
        params![id, summary, model],
    )
    .context("saving a hub summary")?;
    Ok(())
}

/// Summarizes one waiting meeting, if there is one. Returns its id.
pub fn run_once(
    conn: &Connection,
    llm: &dyn Llm,
    model: &str,
    now_ms: i64,
    delay_ms: i64,
) -> Result<Option<String>> {
    let Some((id, lines)) = take_next(conn, now_ms, delay_ms)? else {
        return Ok(None);
    };
    let summary = summarize(llm, &lines).with_context(|| format!("summarizing meeting {id}"))?;
    save(conn, &id, &summary, model)?;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, devices, sync};
    use hikaya_core::sync::{MeetingRecord, PushRequest};
    use std::sync::Mutex;

    struct FakeLlm {
        asked: Mutex<Vec<String>>,
        fail: bool,
    }

    impl Llm for FakeLlm {
        fn ask(&self, prompt: &str) -> Result<String> {
            self.asked.lock().expect("lock").push(prompt.to_string());
            if self.fail {
                return Err(anyhow!("Ollama is down"));
            }
            Ok("## TL;DR\nOn se voit jeudi.".into())
        }
    }

    fn fake(fail: bool) -> FakeLlm {
        FakeLlm {
            asked: Mutex::new(Vec::new()),
            fail,
        }
    }

    fn setup() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().expect("open");
        db::migrate(&mut conn).expect("migrate");
        let (laptop, _) = devices::register(&conn, "laptop").expect("laptop");
        (conn, laptop.id)
    }

    fn push(conn: &mut Connection, device: i64, id: &str, summary: Option<&str>, updated: i64) {
        sync::push(
            conn,
            device,
            &PushRequest {
                meetings: vec![MeetingRecord {
                    id: id.into(),
                    title: "Café".into(),
                    started_at: 0,
                    duration_ms: 30_000,
                    updated_at: updated,
                    summary: summary.map(str::to_string),
                    lines: vec![MeetingLineRecord {
                        start_ms: 0,
                        end_ms: 30_000,
                        speaker: "room".into(),
                        text: "On se voit jeudi.".into(),
                    }],
                }],
                ..Default::default()
            },
        )
        .expect("push");
    }

    fn received_long_ago(conn: &Connection) {
        conn.execute("UPDATE meetings SET received_at = 0", [])
            .expect("age");
    }

    #[test]
    fn a_meeting_without_summary_is_summarized_once_it_has_waited() {
        let (mut conn, laptop) = setup();
        push(&mut conn, laptop, "m1", None, 1);
        push(&mut conn, laptop, "m2", Some("## TL;DR\nDéjà fait."), 1);
        let llm = fake(false);
        let now = 10_000_000_000;

        // Just received: the computer may still summarize it.
        assert_eq!(
            run_once(&conn, &llm, "qwen", now, 30 * 60_000).expect("run"),
            None
        );
        received_long_ago(&conn);
        assert_eq!(
            run_once(&conn, &llm, "qwen", now, 30 * 60_000).expect("run"),
            Some("m1".into())
        );
        let asked = llm.asked.lock().expect("lock");
        assert_eq!(asked.len(), 1);
        assert!(asked[0].contains("[00:00] Room: On se voit jeudi."));
        drop(asked);

        let detail = crate::meetings::detail(&conn, "m1").expect("d").expect("f");
        assert_eq!(
            detail.summary.as_deref(),
            Some("## TL;DR\nOn se voit jeudi.")
        );
        assert!(detail.summary_by_hub);
        assert!(detail.meeting.has_summary);
        // Nothing left: m2 has its computer's summary.
        assert_eq!(run_once(&conn, &llm, "qwen", now, 0).expect("run"), None);
    }

    #[test]
    fn the_computers_summary_wins_over_the_hubs() {
        let (mut conn, laptop) = setup();
        push(&mut conn, laptop, "m1", None, 1);
        received_long_ago(&conn);
        run_once(&conn, &fake(false), "qwen", 10_000_000_000, 0).expect("run");
        push(&mut conn, laptop, "m1", Some("## TL;DR\nVersion du PC."), 2);
        let detail = crate::meetings::detail(&conn, "m1").expect("d").expect("f");
        assert_eq!(detail.summary.as_deref(), Some("## TL;DR\nVersion du PC."));
        assert!(!detail.summary_by_hub);
    }

    #[test]
    fn a_failure_waits_a_day_before_the_next_try() {
        let (mut conn, laptop) = setup();
        push(&mut conn, laptop, "m1", None, 1);
        received_long_ago(&conn);
        let now = 10_000_000_000;
        assert!(run_once(&conn, &fake(true), "qwen", now, 0).is_err());
        assert_eq!(
            run_once(&conn, &fake(false), "qwen", now + 60_000, 0).expect("run"),
            None
        );
        assert_eq!(
            run_once(&conn, &fake(false), "qwen", now + RETRY_AFTER_MS, 0).expect("run"),
            Some("m1".into())
        );
    }
}
