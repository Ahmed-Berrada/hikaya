//! Telling speakers apart (ADR 0005, beta, off by default).
//!
//! After a meeting, the "Others" track (everyone the computer played) is
//! split by voice with pyannote community-1 through `speakrs`, then each
//! turn is transcribed on its own, so lines say "Speaker 2" instead of
//! "Others". The user can name the voices in the meeting view.
//!
//! The models (about 32 MB) are downloaded on first use from a pinned
//! revision and checked byte for byte; nothing is bundled.

use super::segments::SAMPLE_RATE;
use crate::hikaya::store::LearningStore;
use crate::managers::transcription::TranscriptionManager;
use anyhow::{anyhow, bail, Context, Result};
use hikaya_core::diarize::{parse_rttm, turns, voice_count};
use hikaya_core::meeting::{Speaker, TranscriptLine};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// The model repository and the revision these files come from.
const REPO: &str = "avencera/speakrs-models";
const REVISION: &str = "67f963b6a1d94fcd4678948237b3cc8bd69bf1d8";

/// The files the CPU pipeline loads, with their SHA-256.
const FILES: &[(&str, &str)] = &[
    (
        "plda_lda.npy",
        "e20c9b012bebd1aabda5a38a127e63a43cf35debdc502715fc143e2fb6bc3c4b",
    ),
    (
        "plda_mean1.npy",
        "e424c0c352182aa8e0f555dec1f3b30e29a20b9ed6b25d339f112af92e51e36f",
    ),
    (
        "plda_mean2.npy",
        "6f6fb708a2037197b5b84ffeaa8f140cb878088fbecd6ab042ad26a7691bd2cf",
    ),
    (
        "plda_mu.npy",
        "d286d48acf99bbc1ed1502fed0a3e361ae5626ce1870c8be9f7397c5e47886c6",
    ),
    (
        "plda_psi.npy",
        "d7128c9ed2f28a9781971805131129f077c04f948e2df12e52dcdb99f2b4e5f5",
    ),
    (
        "plda_tr.npy",
        "e700b68cb319de3fafb5fa093eb9222c23c447084741f8d3a533640d425510ee",
    ),
    (
        "segmentation-3.0.safetensors",
        "88e7883cf56f679170e162419efddf970f929915281ebce80e351c42b5a7115d",
    ),
    (
        "wespeaker-multimask-tail.safetensors",
        "b97b090ceebcf10924732ad4552a2d867d76d38dcc1409a13687a6ba71ff1ef3",
    ),
    (
        "wespeaker-voxceleb-resnet34.min_num_samples.txt",
        "e4df891c484d7abb985dadf539fa1883a646dab6337af5cae4159c587b7050cc",
    ),
];

/// Meetings shorter than this are not worth splitting.
const MIN_TRACK_SECONDS: usize = 20;
/// Longer tracks are not split: the whole track is held in memory (about
/// 230 MB an hour) while the model works.
const MAX_TRACK_SECONDS: usize = 3 * 60 * 60;

/// One split at a time: it is heavy on the CPU, and a second meeting can
/// wait for the first.
static BUSY: Mutex<()> = Mutex::new(());

/// Where the models live.
fn models_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)?
        .join("hikaya-models")
        .join(format!("speakrs-{}", &REVISION[..12])))
}

fn sha256(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Downloads any model file that is missing or damaged.
fn ensure_models(app: &AppHandle) -> Result<PathBuf> {
    let dir = models_dir(app)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, expected) in FILES {
        let path = dir.join(name);
        if path.exists() && sha256(&path)? == *expected {
            continue;
        }
        let url = format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{name}");
        log::info!("Downloading the speaker model file {name}");
        let bytes = tauri::async_runtime::block_on(async {
            let response = reqwest::get(&url).await?.error_for_status()?;
            response.bytes().await
        })
        .with_context(|| format!("downloading {name}"))?;
        let got = format!("{:x}", Sha256::digest(&bytes));
        if got != *expected {
            bail!("{name} did not match its checksum; nothing was saved");
        }
        let partial = dir.join(format!("{name}.part"));
        std::fs::write(&partial, &bytes).with_context(|| format!("saving {name}"))?;
        std::fs::rename(&partial, &path).with_context(|| format!("saving {name}"))?;
    }
    Ok(dir)
}

/// The whole "Others" track, from its 30-second segments, with silence for
/// a missing one so times stay right.
fn read_track(dir: &Path) -> Result<Vec<f32>> {
    let mut indexed: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let index = name
                .strip_prefix("others-")?
                .strip_suffix(".wav")?
                .parse()
                .ok()?;
            Some((index, path))
        })
        .collect();
    indexed.sort();
    let segment = (SAMPLE_RATE * super::segments::SEGMENT_SECONDS) as usize;
    let mut audio = Vec::new();
    for (index, path) in indexed {
        let start = index as usize * segment;
        if audio.len() < start {
            audio.resize(start, 0.0);
        }
        let mut reader =
            hound::WavReader::open(&path).with_context(|| format!("opening {}", path.display()))?;
        for sample in reader.samples::<i16>() {
            audio.push(f32::from(sample?) / f32::from(i16::MAX));
        }
    }
    Ok(audio)
}

fn ms_to_sample(ms: i64) -> usize {
    (ms.max(0) as usize) * SAMPLE_RATE as usize / 1000
}

/// The turns of each voice in a 16 kHz mono track.
fn separate(audio: &[f32], models: &Path) -> Result<Vec<hikaya_core::diarize::Turn>> {
    let mut pipeline =
        speakrs::OwnedDiarizationPipeline::from_dir(models, speakrs::ExecutionMode::Cpu)
            .map_err(|e| anyhow!("loading the speaker models: {e}"))?;
    let result = pipeline
        .run(audio)
        .map_err(|e| anyhow!("telling the speakers apart: {e}"))?;
    Ok(turns(&parse_rttm(&result.rttm("meeting"))))
}

/// Splits a finished meeting's "Others" track by speaker and replaces its
/// lines. Returns how many voices were found; with fewer than two the
/// transcript is left as it was.
pub fn split(app: &AppHandle, store: &LearningStore, id: i64, audio_dir: &Path) -> Result<u32> {
    let _one_at_a_time = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let audio = read_track(audio_dir)?;
    if audio.len() < MIN_TRACK_SECONDS * SAMPLE_RATE as usize {
        return Ok(0);
    }
    if audio.len() > MAX_TRACK_SECONDS * SAMPLE_RATE as usize {
        bail!("the meeting is longer than three hours");
    }
    let models = ensure_models(app)?;
    let started = std::time::Instant::now();
    let turns = separate(&audio, &models)?;
    let voices = voice_count(&turns);
    log::info!(
        "Meeting {id}: {voices} voice(s) in {:.0} s of audio, found in {:.0} s",
        audio.len() as f64 / f64::from(SAMPLE_RATE),
        started.elapsed().as_secs_f64()
    );
    if voices < 2 {
        return Ok(voices);
    }

    let tm = app
        .try_state::<Arc<TranscriptionManager>>()
        .ok_or_else(|| anyhow!("transcription is unavailable"))?;
    let control = super::manager::GiveWayToDictation { app };
    let mut lines = Vec::new();
    for turn in &turns {
        let from = ms_to_sample(turn.start_ms).min(audio.len());
        let to = ms_to_sample(turn.end_ms).min(audio.len());
        let samples = audio[from..to].to_vec();
        if !hikaya_core::audio::has_speech(&samples) {
            continue;
        }
        let Some(text) = tm
            .transcribe_in_chunks(samples, &control)
            .with_context(|| format!("transcribing a turn at {} ms", turn.start_ms))?
        else {
            continue;
        };
        let text = crate::hikaya::stage::apply(app, text.text);
        if text.trim().is_empty() {
            continue;
        }
        lines.push(TranscriptLine {
            start_ms: turn.start_ms,
            end_ms: turn.end_ms,
            speaker: Speaker::Others,
            text,
            voice: Some(turn.voice),
        });
    }
    if lines.is_empty() {
        // Nothing came out of the turns: keep the transcript as it was.
        bail!("no turn could be transcribed");
    }
    store.replace_track_lines(id, Speaker::Others, &lines)?;
    Ok(voices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_segment_becomes_silence_so_times_stay_right() {
        let dir = tempfile::tempdir().expect("dir");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // Segment 0 is missing (deleted, or the track started late); 1 has
        // one second; a microphone file is ignored.
        for name in ["others-00001.wav", "you-00000.wav"] {
            let mut w = hound::WavWriter::create(dir.path().join(name), spec).expect("wav");
            for _ in 0..SAMPLE_RATE {
                w.write_sample(i16::MAX / 2).expect("sample");
            }
            w.finalize().expect("finish");
        }
        let audio = read_track(dir.path()).expect("track");
        let segment = (SAMPLE_RATE * super::super::segments::SEGMENT_SECONDS) as usize;
        assert_eq!(audio.len(), segment + SAMPLE_RATE as usize);
        assert_eq!(audio[0], 0.0);
        assert!(audio[segment] > 0.4);
    }

    #[test]
    fn the_pinned_files_are_the_ones_the_pipeline_loads() {
        assert_eq!(FILES.len(), 9);
        assert!(FILES.iter().all(|(_, hash)| hash.len() == 64));
        assert!(FILES
            .iter()
            .any(|(name, _)| *name == "segmentation-3.0.safetensors"));
    }

    /// The real model on a real meeting, cut into the app's segments:
    /// `HIKAYA_SPEAKRS_MODELS=<dir with the 9 files> HIKAYA_AMI_WAV=ES2004a.wav
    /// cargo test --release --lib speakers -- --ignored`
    #[test]
    #[ignore = "needs the speaker models and an AMI meeting"]
    fn finds_the_speakers_of_a_real_meeting() {
        let models = PathBuf::from(std::env::var("HIKAYA_SPEAKRS_MODELS").expect("models"));
        let wav = std::env::var("HIKAYA_AMI_WAV").expect("wav");
        for (name, hash) in FILES {
            assert_eq!(sha256(&models.join(name)).expect("read"), *hash, "{name}");
        }
        let mut reader = hound::WavReader::open(&wav).expect("wav");
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .take(SAMPLE_RATE as usize * 300)
            .map(|s| s.expect("sample"))
            .collect();
        let dir = tempfile::tempdir().expect("dir");
        let spec = reader.spec();
        let segment = (SAMPLE_RATE * super::super::segments::SEGMENT_SECONDS) as usize;
        for (i, chunk) in samples.chunks(segment).enumerate() {
            let path = dir.path().join(format!("others-{i:05}.wav"));
            let mut w = hound::WavWriter::create(path, spec).expect("wav");
            for s in chunk {
                w.write_sample(*s).expect("sample");
            }
            w.finalize().expect("finish");
        }
        let audio = read_track(dir.path()).expect("track");
        assert_eq!(audio.len(), samples.len());
        let started = std::time::Instant::now();
        let found = separate(&audio, &models).expect("separate");
        eprintln!(
            "{} turns, {} voices, in {:.0} s for 300 s of audio",
            found.len(),
            voice_count(&found),
            started.elapsed().as_secs_f64()
        );
        assert!(voice_count(&found) >= 3, "AMI ES2004a has four speakers");
    }
}
