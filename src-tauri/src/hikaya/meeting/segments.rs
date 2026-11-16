//! Writes one track of a meeting as consecutive WAV segments.
//!
//! Each segment holds `SEGMENT_SECONDS` of 16 kHz mono audio. When one is
//! full it is finished on disk and handed on for transcription, so a crash
//! loses at most the segment being written, and transcripts arrive while the
//! meeting is still going.

use anyhow::{Context, Result};
use hikaya_core::meeting::Speaker;
use std::path::{Path, PathBuf};

/// Sample rate of every segment.
pub const SAMPLE_RATE: u32 = 16_000;
/// Length of a full segment.
pub const SEGMENT_SECONDS: u32 = 30;
const SEGMENT_SAMPLES: usize = (SAMPLE_RATE * SEGMENT_SECONDS) as usize;

/// A finished segment, ready to transcribe.
#[derive(Debug, Clone)]
pub struct Segment {
    pub speaker: Speaker,
    /// 0 for the first segment of the track.
    pub index: u32,
    /// Offset from the start of the meeting.
    pub start_ms: i64,
    pub samples: Vec<f32>,
    pub path: PathBuf,
}

fn file_name(speaker: Speaker, index: u32) -> String {
    let track = match speaker {
        Speaker::You => "you",
        Speaker::Others => "others",
    };
    format!("{track}-{index:05}.wav")
}

/// Writes the audio of one track.
pub struct SegmentWriter {
    dir: PathBuf,
    speaker: Speaker,
    index: u32,
    /// Samples of the segment being filled; written to disk as it grows.
    buffer: Vec<f32>,
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    on_segment: Box<dyn FnMut(Segment) + Send>,
}

impl SegmentWriter {
    /// A writer for `speaker`'s track in `dir`. `on_segment` is called on the
    /// writing thread with each finished segment; it should hand the segment
    /// on and return quickly.
    pub fn new(
        dir: &Path,
        speaker: Speaker,
        on_segment: impl FnMut(Segment) + Send + 'static,
    ) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            speaker,
            index: 0,
            buffer: Vec::with_capacity(SEGMENT_SAMPLES),
            writer: None,
            on_segment: Box::new(on_segment),
        })
    }

    fn open(&mut self) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let path = self.dir.join(file_name(self.speaker, self.index));
        self.writer = Some(
            hound::WavWriter::create(&path, spec)
                .with_context(|| format!("creating {}", path.display()))?,
        );
        Ok(())
    }

    /// Appends 16 kHz mono samples, finishing segments as they fill.
    pub fn push(&mut self, mut samples: &[f32]) -> Result<()> {
        while !samples.is_empty() {
            if self.writer.is_none() {
                self.open()?;
            }
            let room = SEGMENT_SAMPLES - self.buffer.len();
            let (now, later) = samples.split_at(room.min(samples.len()));
            if let Some(writer) = self.writer.as_mut() {
                for &s in now {
                    writer.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
                }
                // Keeps the file usable up to here if the app stops abruptly.
                writer.flush()?;
            }
            self.buffer.extend_from_slice(now);
            samples = later;
            if self.buffer.len() == SEGMENT_SAMPLES {
                self.finish_segment()?;
            }
        }
        Ok(())
    }

    fn finish_segment(&mut self) -> Result<()> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        writer.finalize().context("finishing a segment")?;
        let samples = std::mem::replace(&mut self.buffer, Vec::with_capacity(SEGMENT_SAMPLES));
        let segment = Segment {
            speaker: self.speaker,
            index: self.index,
            start_ms: i64::from(self.index) * i64::from(SEGMENT_SECONDS) * 1000,
            samples,
            path: self.dir.join(file_name(self.speaker, self.index)),
        };
        self.index += 1;
        (self.on_segment)(segment);
        Ok(())
    }

    /// Finishes the last, partial segment. Returns the track's length.
    pub fn finish(mut self) -> Result<i64> {
        let partial = self.buffer.len();
        if partial > 0 {
            self.finish_segment()?;
        } else if let Some(writer) = self.writer.take() {
            writer.finalize()?;
        }
        let full = i64::from(self.index.saturating_sub(u32::from(partial > 0)));
        Ok(full * i64::from(SEGMENT_SECONDS) * 1000
            + partial as i64 * 1000 / i64::from(SAMPLE_RATE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn writer(dir: &Path) -> (SegmentWriter, Arc<Mutex<Vec<Segment>>>) {
        let done = Arc::new(Mutex::new(Vec::new()));
        let sink = done.clone();
        let writer = SegmentWriter::new(dir, Speaker::Others, move |s| {
            sink.lock().expect("lock").push(s)
        })
        .expect("writer");
        (writer, done)
    }

    #[test]
    fn audio_is_cut_into_thirty_second_files() {
        let dir = tempfile::tempdir().expect("dir");
        let (mut w, done) = writer(dir.path());
        // 65 s of audio in uneven chunks.
        let total = SAMPLE_RATE as usize * 65;
        let audio: Vec<f32> = (0..total)
            .map(|i| ((i % 100) as f32 / 100.0) - 0.5)
            .collect();
        for chunk in audio.chunks(4_801) {
            w.push(chunk).expect("push");
        }
        assert_eq!(
            done.lock().expect("lock").len(),
            2,
            "two full segments so far"
        );
        let length = w.finish().expect("finish");
        assert_eq!(length, 65_000);

        let segments = done.lock().expect("lock");
        let starts: Vec<i64> = segments.iter().map(|s| s.start_ms).collect();
        assert_eq!(starts, [0, 30_000, 60_000]);
        assert_eq!(segments[2].samples.len(), SAMPLE_RATE as usize * 5);
        for s in segments.iter() {
            let reader = hound::WavReader::open(&s.path).expect("valid wav");
            assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
            assert_eq!(reader.len() as usize, s.samples.len());
        }
        assert!(dir.path().join("others-00002.wav").is_file());
    }

    #[test]
    fn exactly_full_segments_leave_no_empty_file() {
        let dir = tempfile::tempdir().expect("dir");
        let (mut w, done) = writer(dir.path());
        w.push(&vec![0.1; SEGMENT_SAMPLES]).expect("push");
        assert_eq!(w.finish().expect("finish"), 30_000);
        assert_eq!(done.lock().expect("lock").len(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 1);
    }

    #[test]
    fn a_track_with_no_audio_has_no_segments() {
        let dir = tempfile::tempdir().expect("dir");
        let (w, done) = writer(dir.path());
        assert_eq!(w.finish().expect("finish"), 0);
        assert!(done.lock().expect("lock").is_empty());
    }

    #[test]
    fn a_segment_being_written_is_readable_after_a_crash() {
        let dir = tempfile::tempdir().expect("dir");
        let (mut w, _done) = writer(dir.path());
        w.push(&vec![0.25; SAMPLE_RATE as usize * 3]).expect("push");
        // Simulate the app dying: never call finish, never drop cleanly.
        std::mem::forget(w);
        let reader = hound::WavReader::open(dir.path().join("others-00000.wav"));
        assert!(reader.is_ok(), "the flushed header keeps the file readable");
    }

    #[test]
    fn samples_are_clipped_not_wrapped() {
        let dir = tempfile::tempdir().expect("dir");
        let (mut w, done) = writer(dir.path());
        w.push(&[2.0, -2.0]).expect("push");
        w.finish().expect("finish");
        let path = done.lock().expect("lock")[0].path.clone();
        let samples: Vec<i16> = hound::WavReader::open(path)
            .expect("open")
            .into_samples()
            .collect::<Result<_, _>>()
            .expect("read");
        assert_eq!(samples, [i16::MAX, -i16::MAX]);
    }
}
