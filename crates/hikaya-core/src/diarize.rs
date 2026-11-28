//! Turning a speaker-separation result into turns to transcribe.
//!
//! A meeting's "Others" track is first transcribed in 30-second segments,
//! which may hold several voices. After the meeting, a speaker-separation
//! model (pyannote community-1, through `speakrs`) says who spoke when; the
//! app then transcribes each turn on its own, so each line has one voice.
//! See ADR 0005.

/// One stretch of one voice, as the separation model reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSpan {
    /// Start, in milliseconds from the start of the track.
    pub start_ms: i64,
    /// End, in milliseconds from the start of the track.
    pub end_ms: i64,
    /// The model's name for the voice, such as `SPEAKER_01`.
    pub label: String,
}

/// One turn to transcribe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    /// Start, in milliseconds from the start of the track.
    pub start_ms: i64,
    /// End, in milliseconds from the start of the track.
    pub end_ms: i64,
    /// The voice, numbered from 1 in the order the voices are first heard.
    pub voice: u32,
}

/// Spans of the same voice closer than this are one turn.
pub const MERGE_GAP_MS: i64 = 1_000;
/// Shorter turns are dropped: too short to transcribe, often a cough.
pub const MIN_TURN_MS: i64 = 400;
/// Longer turns are cut, as the speech model works on 30-second windows.
pub const MAX_TURN_MS: i64 = 30_000;

/// Merges a voice's close spans into turns, numbers the voices by first
/// appearance, cuts long turns and drops tiny ones. Turns come out in time
/// order; two voices talking over each other give two overlapping turns.
pub fn turns(spans: &[VoiceSpan]) -> Vec<Turn> {
    let mut spans: Vec<&VoiceSpan> = spans.iter().filter(|s| s.end_ms > s.start_ms).collect();
    spans.sort_by_key(|s| (s.start_ms, s.end_ms));
    let mut numbers: Vec<&str> = Vec::new();
    let mut open: Vec<Turn> = Vec::new();
    let mut done: Vec<Turn> = Vec::new();
    for span in spans {
        let voice = match numbers.iter().position(|l| *l == span.label) {
            Some(i) => i,
            None => {
                numbers.push(&span.label);
                numbers.len() - 1
            }
        } as u32
            + 1;
        match open.iter_mut().find(|t| t.voice == voice) {
            Some(turn) if span.start_ms - turn.end_ms <= MERGE_GAP_MS => {
                turn.end_ms = turn.end_ms.max(span.end_ms);
            }
            Some(turn) => {
                done.push(*turn);
                *turn = Turn {
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                    voice,
                };
            }
            None => open.push(Turn {
                start_ms: span.start_ms,
                end_ms: span.end_ms,
                voice,
            }),
        }
    }
    done.extend(open);
    let mut out: Vec<Turn> = Vec::new();
    for turn in done {
        if turn.end_ms - turn.start_ms < MIN_TURN_MS {
            continue;
        }
        let mut start = turn.start_ms;
        while start < turn.end_ms {
            let end = (start + MAX_TURN_MS).min(turn.end_ms);
            // A tail shorter than the minimum joins the piece before it.
            let end = if turn.end_ms - end < MIN_TURN_MS {
                turn.end_ms
            } else {
                end
            };
            out.push(Turn {
                start_ms: start,
                end_ms: end,
                voice: turn.voice,
            });
            start = end;
        }
    }
    out.sort_by_key(|t| (t.start_ms, t.voice));
    // Number again in order of appearance: a voice whose only turns were
    // dropped must not leave a gap ("Speaker 1, Speaker 3").
    let mut seen: Vec<u32> = Vec::new();
    for turn in &mut out {
        let number = match seen.iter().position(|v| *v == turn.voice) {
            Some(i) => i,
            None => {
                seen.push(turn.voice);
                seen.len() - 1
            }
        };
        turn.voice = number as u32 + 1;
    }
    out
}

/// Reads RTTM, the usual speaker-separation output: one
/// `SPEAKER <file> <channel> <start s> <duration s> <NA> <NA> <label> ...`
/// line per span. Other lines and unreadable numbers are skipped.
pub fn parse_rttm(text: &str) -> Vec<VoiceSpan> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.first() != Some(&"SPEAKER") || fields.len() < 8 {
                return None;
            }
            let start: f64 = fields[3].parse().ok()?;
            let duration: f64 = fields[4].parse().ok()?;
            if !start.is_finite() || !duration.is_finite() || start < 0.0 || duration <= 0.0 {
                return None;
            }
            Some(VoiceSpan {
                start_ms: (start * 1000.0).round() as i64,
                end_ms: ((start + duration) * 1000.0).round() as i64,
                label: fields[7].to_string(),
            })
        })
        .collect()
}

/// How many voices the turns hold.
pub fn voice_count(turns: &[Turn]) -> u32 {
    turns.iter().map(|t| t.voice).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_s: f64, end_s: f64, label: &str) -> VoiceSpan {
        VoiceSpan {
            start_ms: (start_s * 1000.0) as i64,
            end_ms: (end_s * 1000.0) as i64,
            label: label.into(),
        }
    }

    #[test]
    fn voices_are_numbered_by_first_appearance_and_close_spans_merge() {
        let t = turns(&[
            span(0.0, 4.0, "SPEAKER_03"),
            span(4.5, 6.0, "SPEAKER_03"),
            span(6.2, 9.0, "SPEAKER_00"),
            span(9.1, 12.0, "SPEAKER_03"),
        ]);
        assert_eq!(
            t,
            [
                Turn {
                    start_ms: 0,
                    end_ms: 6_000,
                    voice: 1
                },
                Turn {
                    start_ms: 6_200,
                    end_ms: 9_000,
                    voice: 2
                },
                Turn {
                    start_ms: 9_100,
                    end_ms: 12_000,
                    voice: 1
                },
            ]
        );
        assert_eq!(voice_count(&t), 2);
    }

    #[test]
    fn long_turns_are_cut_and_tiny_ones_dropped() {
        let t = turns(&[
            span(0.0, 0.2, "C"),
            span(0.5, 65.0, "A"),
            span(70.0, 70.2, "B"),
        ]);
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].start_ms, t[0].end_ms), (500, 30_500));
        assert_eq!((t[2].start_ms, t[2].end_ms), (60_500, 65_000));
        // C and B said too little to keep: A is the only voice, numbered 1.
        assert_eq!(voice_count(&t), 1);
        assert!(t.iter().all(|turn| turn.voice == 1));
        // A tail too short to transcribe joins the piece before it.
        let t = turns(&[span(0.0, 30.2, "A")]);
        assert_eq!(
            t,
            [Turn {
                start_ms: 0,
                end_ms: 30_200,
                voice: 1
            }]
        );
    }

    #[test]
    fn overlapping_voices_give_overlapping_turns() {
        let t = turns(&[span(0.0, 5.0, "A"), span(3.0, 8.0, "B")]);
        assert_eq!(t.len(), 2);
        assert!(t[1].start_ms < t[0].end_ms);
    }

    #[test]
    fn rttm_is_read_and_junk_skipped() {
        let spans = parse_rttm(
            "SPEAKER m 1 0.500 2.250 <NA> <NA> SPEAKER_00 <NA> <NA>\n\
             SPEAKER m 1 x 1 <NA> <NA> SPEAKER_01 <NA> <NA>\n\
             SPEAKER m 1 3.0 -1 <NA> <NA> SPEAKER_01 <NA> <NA>\n\
             garbage\n\
             SPEAKER m 1 3.0 1.5 <NA> <NA> SPEAKER_01 <NA> <NA>",
        );
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start_ms, spans[0].end_ms), (500, 2_750));
        assert_eq!(spans[1].label, "SPEAKER_01");
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(turns(&[]).is_empty());
        assert!(turns(&[span(2.0, 2.0, "A")]).is_empty());
    }

    proptest::proptest! {
        #[test]
        fn turns_are_ordered_bounded_and_inside_the_spans(
            raw in proptest::collection::vec((0i64..600_000, 0i64..90_000, 0u8..4), 0..40)
        ) {
            let spans: Vec<VoiceSpan> = raw
                .iter()
                .map(|(start, len, who)| VoiceSpan {
                    start_ms: *start,
                    end_ms: start + len,
                    label: format!("S{who}"),
                })
                .collect();
            let t = turns(&spans);
            let lo = spans.iter().map(|s| s.start_ms).min().unwrap_or(0);
            let hi = spans.iter().map(|s| s.end_ms).max().unwrap_or(0);
            for pair in t.windows(2) {
                proptest::prop_assert!(pair[0].start_ms <= pair[1].start_ms);
            }
            for turn in &t {
                proptest::prop_assert!(turn.end_ms - turn.start_ms >= MIN_TURN_MS);
                proptest::prop_assert!(turn.end_ms - turn.start_ms < MAX_TURN_MS + MIN_TURN_MS);
                proptest::prop_assert!(turn.start_ms >= lo && turn.end_ms <= hi);
                proptest::prop_assert!(turn.voice >= 1 && turn.voice <= 4);
            }
        }
    }
}
