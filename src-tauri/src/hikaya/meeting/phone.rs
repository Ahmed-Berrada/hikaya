//! Turns recordings made on the phone into meetings (ADR 0004).
//!
//! After each successful sync, this computer claims the oldest recording the
//! phone left on the hub, downloads it, transcribes it the way meetings are
//! transcribed, and saves the meeting. The next sync sends the meeting; once
//! the hub has it, the recording is marked complete and its audio deleted.
//! A failure gives the recording back to the hub with the reason.

use super::manager::{GiveWayToDictation, UPDATED_EVENT};
use super::segments::{SAMPLE_RATE, SEGMENT_SECONDS};
use super::summary::{summarize, SummaryModel};
use crate::hikaya::store::LearningStore;
use crate::hikaya::sync::{HttpHub, HubConfig, PhoneRecording};
use crate::managers::transcription::TranscriptionManager;
use anyhow::{anyhow, Context, Result};
use hikaya_core::meeting::{Speaker, SpeakerNames, TranscriptLine};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Recordings handled per pass, so a backlog does not hold the GPU for hours
/// without a sync in between.
const PER_PASS: usize = 3;
const SEGMENT_SAMPLES: usize = (SAMPLE_RATE * SEGMENT_SECONDS) as usize;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts a pass in the background, unless one is running.
pub fn check(app: &AppHandle, config: &HubConfig) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (app, config) = (app.clone(), config.clone());
    let spawned = std::thread::Builder::new()
        .name("hikaya-phone-recordings".into())
        .spawn(move || {
            if let Err(e) = pass(&app, &config) {
                log::warn!("Phone recordings: {e:#}");
            }
            RUNNING.store(false, Ordering::SeqCst);
        });
    if let Err(e) = spawned {
        log::error!("Could not start the phone recordings worker: {e}");
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

/// Why this computer should not take a recording now, if it should not.
fn busy_reason(app: &AppHandle) -> Option<&'static str> {
    if crate::settings::get_settings(app).selected_model.is_empty() {
        return Some("no speech model is chosen");
    }
    if crate::hikaya::meeting_in_progress(app) {
        return Some("a meeting is being recorded");
    }
    None
}

fn pass(app: &AppHandle, config: &HubConfig) -> Result<()> {
    let store = app
        .try_state::<Arc<LearningStore>>()
        .context("the learning store is unavailable")?
        .inner()
        .clone();
    let hub = HttpHub::for_config(config)?;
    complete_sent(&store, &hub);
    if let Some(why) = busy_reason(app) {
        log::debug!("Not taking phone recordings: {why}");
        return Ok(());
    }
    for _ in 0..PER_PASS {
        let Some(recording) = block_on(hub.claim_recording())? else {
            break;
        };
        log::info!(
            "Transcribing phone recording {} ({} KB)",
            recording.id,
            recording.bytes / 1024
        );
        match transcribe(app, &store, &hub, &recording) {
            Ok(meeting) => {
                log::info!("Phone recording {} is meeting {meeting}", recording.id);
                if let Some(sync) = app.try_state::<Arc<crate::hikaya::sync_worker::SyncWorker>>() {
                    // Sends the meeting; the hub then accepts the completion.
                    block_on(sync.run());
                }
                complete_sent(&store, &hub);
            }
            Err(e) => {
                log::warn!("Phone recording {} failed: {e:#}", recording.id);
                if let Err(e) = block_on(hub.fail_recording(&recording.id, &format!("{e:#}"))) {
                    log::warn!("Could not report the failure to the hub: {e}");
                }
            }
        }
    }
    Ok(())
}

/// Tells the hub about each phone meeting it has received.
fn complete_sent(store: &LearningStore, hub: &HttpHub) {
    let pending = match store.phone_completions() {
        Ok(pending) => pending,
        Err(e) => return log::warn!("Could not read phone meetings: {e:#}"),
    };
    for (recording, meeting_uid) in pending {
        match block_on(hub.complete_recording(&recording, &meeting_uid)) {
            Ok(()) => {
                if let Err(e) = store.mark_phone_completed(&recording) {
                    log::warn!("Could not note recording {recording} as complete: {e:#}");
                }
            }
            Err(e) => log::warn!("The hub did not complete recording {recording}: {e}"),
        }
    }
}

/// The file extension the decoder expects for an audio type.
fn extension(mime: &str) -> &'static str {
    match mime {
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        _ => "wav",
    }
}

/// Splits audio into the 30-second segments meetings are transcribed in:
/// `(start in ms, samples)`.
fn segments(samples: &[f32]) -> impl Iterator<Item = (i64, &[f32])> {
    samples
        .chunks(SEGMENT_SAMPLES)
        .enumerate()
        .map(|(i, part)| (ms(i * SEGMENT_SAMPLES), part))
}

fn ms(samples: usize) -> i64 {
    samples as i64 * 1000 / i64::from(SAMPLE_RATE)
}

fn speaker_names(app: &AppHandle) -> SpeakerNames {
    let french = crate::settings::get_settings(app)
        .app_language
        .to_lowercase()
        .starts_with("fr");
    let (you, others, room) = if french {
        ("Vous", "Autres", "Salle")
    } else {
        ("You", "Others", "Room")
    };
    SpeakerNames {
        you: you.into(),
        others: others.into(),
        room: room.into(),
    }
}

/// Downloads, decodes and transcribes one recording. Returns the meeting.
fn transcribe(
    app: &AppHandle,
    store: &LearningStore,
    hub: &HttpHub,
    recording: &PhoneRecording,
) -> Result<i64> {
    let audio = block_on(hub.recording_audio(&recording.id))?;
    let dir = crate::portable::app_data_dir(app)?.join("phone");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.{}", recording.id, extension(&recording.mime)));
    std::fs::write(&path, &audio).with_context(|| format!("writing {}", path.display()))?;
    let decoded = crate::audio_toolkit::audio::decode_audio_file(&path);
    let _ = std::fs::remove_file(&path);
    let samples = decoded.context("the recording could not be read")?;

    let tm = app
        .try_state::<Arc<TranscriptionManager>>()
        .ok_or_else(|| anyhow!("transcription is unavailable"))?;
    let control = GiveWayToDictation { app };
    let mut lines = Vec::new();
    for (start_ms, part) in segments(&samples) {
        // Whisper makes up words for silence; quiet segments are skipped.
        if !hikaya_core::audio::has_speech(part) {
            continue;
        }
        let Some(text) = tm.transcribe_in_chunks(part.to_vec(), &control)? else {
            continue;
        };
        let text = crate::hikaya::stage::apply(app, text.text);
        if text.trim().is_empty() {
            continue;
        }
        lines.push(TranscriptLine {
            start_ms,
            end_ms: start_ms + ms(part.len()),
            speaker: Speaker::Room,
            text,
        });
    }
    let meeting = store.save_phone_meeting(
        &recording.id,
        &recording.title,
        recording.created_at,
        ms(samples.len()),
        &lines,
    )?;
    // The phone cannot summarize, so the computer does it when it can.
    if !lines.is_empty() {
        match SummaryModel::from_settings(&crate::settings::get_settings(app)) {
            Ok(model) => match block_on(summarize(&model, &lines, &speaker_names(app))) {
                Ok(summary) => store.set_meeting_summary(meeting, &summary, model.name())?,
                Err(e) => log::warn!("Meeting {meeting} not summarized: {e}"),
            },
            Err(why) => log::info!("Meeting {meeting} not summarized: {why}"),
        }
    }
    let _ = app.emit(UPDATED_EVENT, meeting);
    Ok(meeting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_types_map_to_extensions_the_decoder_knows() {
        assert_eq!(extension("audio/mp4"), "m4a");
        assert_eq!(extension("audio/webm"), "webm");
        assert_eq!(extension("audio/ogg"), "ogg");
        assert_eq!(extension("audio/wav"), "wav");
    }

    #[test]
    fn audio_is_cut_into_thirty_second_segments_with_their_start() {
        let samples = vec![0.0; SEGMENT_SAMPLES * 2 + 16_000];
        let parts: Vec<(i64, usize)> = segments(&samples).map(|(s, p)| (s, p.len())).collect();
        assert_eq!(
            parts,
            [
                (0, SEGMENT_SAMPLES),
                (30_000, SEGMENT_SAMPLES),
                (60_000, 16_000)
            ]
        );
        assert_eq!(ms(16_000), 1_000);
    }
}
