//! Meeting transcripts: the lines, cleaning up echo between the two tracks,
//! splitting long transcripts for summarizing, and Markdown export.
//!
//! A meeting is recorded as two tracks (see ADR 0003): the microphone, shown
//! as "You", and the system output, shown as "Others".

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Who a line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    /// The microphone track: the person using this device.
    You,
    /// The system audio track: everyone on the other end of a call.
    Others,
}

/// One transcribed stretch of speech.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptLine {
    /// Start, in milliseconds from the start of the meeting.
    pub start_ms: i64,
    /// End, in milliseconds from the start of the meeting.
    pub end_ms: i64,
    /// Which track it came from.
    pub speaker: Speaker,
    /// What was said.
    pub text: String,
}

/// Lines that start this close together count as the same moment.
const ECHO_WINDOW_MS: i64 = 2_000;
/// Share of words two lines must have in common to be the same speech.
const ECHO_SIMILARITY: f64 = 0.6;

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Share of the shorter text's words that also appear in the longer one.
fn overlap(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.is_empty() {
        return 0.0;
    }
    let long: HashSet<&String> = long.iter().collect();
    short.iter().filter(|w| long.contains(w)).count() as f64 / short.len() as f64
}

/// Drops "You" lines that only repeat what the loudspeakers played.
///
/// Without headphones the microphone also records the other people. Such a
/// line has nearly the same words as an "Others" line from the same moment,
/// and belongs to them, not to you.
pub fn remove_echo(lines: Vec<TranscriptLine>) -> Vec<TranscriptLine> {
    let others: Vec<TranscriptLine> = lines
        .iter()
        .filter(|l| l.speaker == Speaker::Others)
        .cloned()
        .collect();
    lines
        .into_iter()
        .filter(|line| {
            line.speaker == Speaker::Others
                || !others.iter().any(|other| {
                    (other.start_ms - line.start_ms).abs() <= ECHO_WINDOW_MS
                        && overlap(&other.text, &line.text) >= ECHO_SIMILARITY
                })
        })
        .collect()
}

/// Sorts lines by time, "You" first when two start together.
pub fn in_order(mut lines: Vec<TranscriptLine>) -> Vec<TranscriptLine> {
    lines.sort_by_key(|l| (l.start_ms, l.speaker == Speaker::Others));
    lines
}

/// `mm:ss`, or `h:mm:ss` past an hour.
pub fn timestamp(ms: i64) -> String {
    let total = ms.max(0) / 1000;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Names to print for each speaker.
#[derive(Clone, Debug)]
pub struct SpeakerNames {
    /// Name for the microphone track, such as "You".
    pub you: String,
    /// Name for the system audio track, such as "Others".
    pub others: String,
}

impl SpeakerNames {
    fn name(&self, speaker: Speaker) -> &str {
        match speaker {
            Speaker::You => &self.you,
            Speaker::Others => &self.others,
        }
    }
}

/// The transcript as plain text, one line per stretch of speech:
/// `[00:42] You: ...`. Consecutive lines by the same speaker are kept apart
/// so timestamps stay accurate.
pub fn plain_transcript(lines: &[TranscriptLine], names: &SpeakerNames) -> String {
    lines
        .iter()
        .map(|l| {
            format!(
                "[{}] {}: {}",
                timestamp(l.start_ms),
                names.name(l.speaker),
                l.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Splits a transcript into parts of at most `max_chars`, cutting only
/// between lines, so a long meeting can be summarized part by part. A single
/// line longer than the limit becomes a part of its own.
pub fn split_for_summary(transcript: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in transcript.lines() {
        if !current.is_empty() && current.len() + 1 + line.len() > max_chars {
            parts.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Everything that goes into a meeting's Markdown export.
#[derive(Clone, Debug)]
pub struct MeetingExport<'a> {
    /// The meeting's title.
    pub title: &'a str,
    /// Human-readable date and time of the start.
    pub started: &'a str,
    /// Length of the recording.
    pub duration_ms: i64,
    /// The summary as Markdown, if one was made.
    pub summary: Option<&'a str>,
    /// The raw transcript, in order.
    pub lines: &'a [TranscriptLine],
    /// How to name each speaker.
    pub names: &'a SpeakerNames,
    /// Heading of the summary section, in the user's language.
    pub summary_heading: &'a str,
    /// Heading of the transcript section.
    pub transcript_heading: &'a str,
}

/// The meeting as a Markdown document: title, date and length, the summary,
/// then the raw transcript.
pub fn to_markdown(export: &MeetingExport<'_>) -> String {
    let mut out = format!(
        "# {}\n\n{} · {}\n",
        export.title.trim(),
        export.started,
        timestamp(export.duration_ms)
    );
    if let Some(summary) = export.summary.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!(
            "\n## {}\n\n{}\n",
            export.summary_heading,
            summary.trim()
        ));
    }
    out.push_str(&format!("\n## {}\n\n", export.transcript_heading));
    for l in export.lines {
        out.push_str(&format!(
            "**{}** {}: {}\n\n",
            timestamp(l.start_ms),
            export.names.name(l.speaker),
            l.text.trim()
        ));
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// The transcript as SubRip subtitles, for video players.
pub fn to_srt(lines: &[TranscriptLine], names: &SpeakerNames) -> String {
    fn srt_time(ms: i64) -> String {
        let ms = ms.max(0);
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3_600_000,
            ms % 3_600_000 / 60_000,
            ms % 60_000 / 1000,
            ms % 1000
        )
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "{}\n{} --> {}\n{}: {}\n",
                i + 1,
                srt_time(l.start_ms),
                srt_time(l.end_ms.max(l.start_ms)),
                names.name(l.speaker),
                l.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start_ms: i64, speaker: Speaker, text: &str) -> TranscriptLine {
        TranscriptLine {
            start_ms,
            end_ms: start_ms + 3_000,
            speaker,
            text: text.into(),
        }
    }

    fn names() -> SpeakerNames {
        SpeakerNames {
            you: "Vous".into(),
            others: "Autres".into(),
        }
    }

    #[test]
    fn echo_of_the_loudspeakers_is_dropped() {
        let lines = vec![
            line(
                10_000,
                Speaker::Others,
                "On décale la démo à jeudi, ça vous va ?",
            ),
            // The microphone heard the same thing, slightly garbled.
            line(10_400, Speaker::You, "on décale la démo jeudi ça vous va"),
            line(14_000, Speaker::You, "Oui, jeudi me va très bien."),
        ];
        let kept = remove_echo(lines);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].text, "Oui, jeudi me va très bien.");
    }

    #[test]
    fn a_real_answer_with_shared_words_is_kept() {
        let lines = vec![
            line(
                10_000,
                Speaker::Others,
                "Can we ship the release on Friday?",
            ),
            line(
                11_000,
                Speaker::You,
                "Yes, Friday works, I'll prepare the notes.",
            ),
        ];
        assert_eq!(remove_echo(lines).len(), 2);
    }

    #[test]
    fn the_same_words_much_later_are_not_echo() {
        let lines = vec![
            line(10_000, Speaker::Others, "let's review the budget"),
            line(60_000, Speaker::You, "let's review the budget"),
        ];
        assert_eq!(remove_echo(lines).len(), 2);
    }

    #[test]
    fn lines_are_ordered_by_time_with_you_first_on_ties() {
        let ordered = in_order(vec![
            line(5_000, Speaker::Others, "b"),
            line(1_000, Speaker::Others, "a"),
            line(5_000, Speaker::You, "c"),
        ]);
        let texts: Vec<&str> = ordered.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["a", "c", "b"]);
    }

    #[test]
    fn timestamps_switch_to_hours_after_an_hour() {
        assert_eq!(timestamp(0), "00:00");
        assert_eq!(timestamp(65_400), "01:05");
        assert_eq!(timestamp(3_725_000), "1:02:05");
        assert_eq!(timestamp(-5), "00:00");
    }

    #[test]
    fn plain_transcript_names_the_speakers() {
        let text = plain_transcript(
            &[
                line(0, Speaker::You, " Bonjour "),
                line(42_000, Speaker::Others, "Hello"),
            ],
            &names(),
        );
        assert_eq!(text, "[00:00] Vous: Bonjour\n[00:42] Autres: Hello");
    }

    #[test]
    fn long_transcripts_are_split_between_lines() {
        let transcript = "aaaa\nbbbb\ncccc\ndddd";
        assert_eq!(
            split_for_summary(transcript, 9),
            ["aaaa\nbbbb", "cccc\ndddd"]
        );
        assert_eq!(split_for_summary(transcript, 1000), [transcript]);
        assert_eq!(split_for_summary("", 10), Vec::<String>::new());
        // A line longer than the limit is never cut.
        assert_eq!(
            split_for_summary("short\nthis line is long\nx", 8),
            ["short", "this line is long", "x"]
        );
    }

    #[test]
    fn markdown_export_has_summary_then_transcript() {
        let lines = [
            line(0, Speaker::You, "Bonjour"),
            line(61_000, Speaker::Others, "Hi"),
        ];
        let names = names();
        let md = to_markdown(&MeetingExport {
            title: "Point produit",
            started: "16 nov. 2026, 14:00",
            duration_ms: 3_725_000,
            summary: Some("- Démo décalée à jeudi"),
            lines: &lines,
            names: &names,
            summary_heading: "Résumé",
            transcript_heading: "Transcription",
        });
        assert_eq!(
            md,
            "# Point produit\n\n16 nov. 2026, 14:00 · 1:02:05\n\n## Résumé\n\n- Démo décalée à jeudi\n\n## Transcription\n\n**00:00** Vous: Bonjour\n\n**01:01** Autres: Hi\n"
        );
    }

    #[test]
    fn markdown_export_without_a_summary_skips_the_section() {
        let names = names();
        let md = to_markdown(&MeetingExport {
            title: "Call",
            started: "today",
            duration_ms: 0,
            summary: Some("  "),
            lines: &[],
            names: &names,
            summary_heading: "Summary",
            transcript_heading: "Transcript",
        });
        assert!(!md.contains("Summary"));
        assert!(md.ends_with("## Transcript\n"));
    }

    #[test]
    fn srt_export_uses_subtitle_timing() {
        let srt = to_srt(
            &[TranscriptLine {
                start_ms: 61_250,
                end_ms: 63_000,
                speaker: Speaker::Others,
                text: "Hi".into(),
            }],
            &names(),
        );
        assert_eq!(srt, "1\n00:01:01,250 --> 00:01:03,000\nAutres: Hi\n");
    }

    proptest::proptest! {
        #[test]
        fn echo_removal_never_drops_other_peoples_lines(texts in proptest::collection::vec("[a-z ]{0,30}", 0..12)) {
            let lines: Vec<TranscriptLine> = texts.iter().enumerate().map(|(i, t)| {
                line(i as i64 * 1000, if i % 2 == 0 { Speaker::Others } else { Speaker::You }, t)
            }).collect();
            let others_before = lines.iter().filter(|l| l.speaker == Speaker::Others).count();
            let kept = remove_echo(lines);
            proptest::prop_assert_eq!(kept.iter().filter(|l| l.speaker == Speaker::Others).count(), others_before);
        }

        #[test]
        fn splitting_keeps_every_line(lines in proptest::collection::vec("[a-z]{0,40}", 0..30), max in 1usize..200) {
            let transcript = lines.join("\n");
            let rejoined = split_for_summary(&transcript, max).join("\n");
            let kept: Vec<&str> = rejoined.lines().filter(|l| !l.is_empty()).collect();
            let original: Vec<&str> = transcript.lines().filter(|l| !l.is_empty()).collect();
            proptest::prop_assert_eq!(kept, original);
        }
    }
}
