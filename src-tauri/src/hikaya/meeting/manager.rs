//! Runs a meeting: starts both tracks, writes their segments, transcribes
//! each finished segment, and reports progress to the UI.
//!
//! Threads per meeting: one writer per track (audio → WAV segments), and one
//! transcriber that takes finished segments in order. Transcription gives
//! way to dictation, which always goes first.

use super::segments::{Segment, SegmentWriter, SAMPLE_RATE};
use super::store::MeetingStatus;
use crate::hikaya::store::LearningStore;
use crate::managers::transcription::{ChunkControl, TranscriptionManager};
use anyhow::{anyhow, Context, Result};
use hikaya_capture::{start_microphone, start_system_audio, TrackCapture};
use hikaya_core::meeting::{Speaker, TranscriptLine};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Event sent to the UI whenever a meeting changes. The payload is its id.
pub const UPDATED_EVENT: &str = "hikaya-meeting-updated";
/// How often the transcriber checks whether dictation has finished.
const DICTATION_POLL: Duration = Duration::from_millis(200);

/// The meeting being recorded, as shown in the UI.
#[derive(Clone, Debug, Serialize, Type)]
pub struct ActiveMeeting {
    pub id: i64,
    pub title: String,
    /// Unix ms.
    pub started_at: i64,
    pub microphone: String,
    /// The system-audio device, or `None` if it could not be opened.
    pub system_audio: Option<String>,
    /// Why system audio is missing, when it is.
    pub system_audio_error: Option<String>,
}

/// A track's audio input and the thread that writes it to segments.
type TrackWriter = (mpsc::Sender<Vec<f32>>, JoinHandle<Result<i64>>);

struct Running {
    info: ActiveMeeting,
    started: Instant,
    tracks: Vec<TrackCapture>,
    writers: Vec<JoinHandle<Result<i64>>>,
    transcriber: JoinHandle<()>,
}

/// Owns the meeting in progress. Registered as Tauri state.
pub struct MeetingManager {
    app: AppHandle,
    store: Arc<LearningStore>,
    running: Mutex<Option<Running>>,
}

/// Lets dictation go first, as file transcription does.
struct GiveWayToDictation<'a> {
    app: &'a AppHandle,
}

impl ChunkControl for GiveWayToDictation<'_> {
    fn proceed(&self) -> bool {
        while crate::tray::dictation_in_progress(self.app) {
            std::thread::sleep(DICTATION_POLL);
        }
        true
    }

    fn progress(&self, _done: usize, _total: usize) {}
}

impl MeetingManager {
    pub fn new(app: &AppHandle, store: Arc<LearningStore>) -> Self {
        // Meetings a crash cut off keep what was saved.
        match store.mark_interrupted_meetings() {
            Ok(ids) if !ids.is_empty() => {
                log::warn!("Marked {} interrupted meeting(s): {ids:?}", ids.len())
            }
            Ok(_) => {}
            Err(e) => log::error!("Could not check for interrupted meetings: {e:#}"),
        }
        Self {
            app: app.clone(),
            store,
            running: Mutex::new(None),
        }
    }

    pub(super) fn app(&self) -> &AppHandle {
        &self.app
    }

    pub(super) fn store(&self) -> &LearningStore {
        &self.store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Where a meeting's audio is kept.
    pub fn audio_dir(&self, id: i64) -> Result<PathBuf> {
        Ok(crate::portable::app_data_dir(&self.app)?
            .join("meetings")
            .join(id.to_string()))
    }

    /// The meeting being recorded, if any.
    pub fn active(&self) -> Option<ActiveMeeting> {
        self.lock().as_ref().map(|r| r.info.clone())
    }

    fn emit(app: &AppHandle, id: i64) {
        if let Err(e) = app.emit(UPDATED_EVENT, id) {
            log::warn!("Could not notify the UI about meeting {id}: {e}");
        }
    }

    /// Starts recording a meeting.
    pub fn start(&self, title: &str) -> Result<ActiveMeeting> {
        let mut running = self.lock();
        if running.is_some() {
            return Err(anyhow!("a meeting is already being recorded"));
        }
        let started_at = chrono::Utc::now().timestamp_millis();
        let title = if title.trim().is_empty() {
            chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
        } else {
            title.trim().to_string()
        };
        let id = self.store.create_meeting(&title, started_at, false)?;
        let dir = self.audio_dir(id)?;
        let settings = crate::settings::get_settings(&self.app);

        let (segment_tx, segment_rx) = mpsc::channel::<Segment>();
        let mut tracks = Vec::new();
        let mut writers = Vec::new();

        let open_track =
            |speaker: Speaker, segment_tx: mpsc::Sender<Segment>| -> Result<TrackWriter> {
                let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>();
                let mut writer = SegmentWriter::new(&dir, speaker, move |segment| {
                    let _ = segment_tx.send(segment);
                })?;
                let handle = std::thread::Builder::new()
                    .name(format!("hikaya-meeting-{speaker:?}"))
                    .spawn(move || -> Result<i64> {
                        for chunk in audio_rx {
                            writer.push(&chunk)?;
                        }
                        writer.finish()
                    })?;
                Ok((audio_tx, handle))
            };

        let (mic_tx, mic_writer) = open_track(Speaker::You, segment_tx.clone())?;
        let microphone = match start_microphone(settings.selected_microphone.as_deref(), mic_tx) {
            Ok(track) => track,
            Err(e) => {
                let _ = self.store.delete_meeting(id);
                return Err(e.context("the microphone could not be opened"));
            }
        };
        let microphone_name = microphone.device_name.clone();
        tracks.push(microphone);
        writers.push(mic_writer);

        let (system_tx, system_writer) = open_track(Speaker::Others, segment_tx)?;
        let (system_audio, system_audio_error) = match start_system_audio(system_tx) {
            Ok(track) => {
                let name = track.device_name.clone();
                tracks.push(track);
                writers.push(system_writer);
                (Some(name), None)
            }
            Err(e) => {
                log::warn!("Meeting {id} records the microphone only: {e:#}");
                // The sender went with the failed capture, so this writer ends at once.
                let _ = system_writer.join();
                (None, Some(format!("{e:#}")))
            }
        };
        if system_audio.is_some() {
            let _ = self.store.conn().and_then(|c| {
                Ok(c.execute("UPDATE meetings SET system_audio = 1 WHERE id = ?1", [id])?)
            });
        }

        let transcriber = {
            let app = self.app.clone();
            let store = self.store.clone();
            std::thread::Builder::new()
                .name("hikaya-meeting-transcriber".into())
                .spawn(move || transcribe_segments(&app, &store, id, segment_rx))
                .context("starting the transcriber")?
        };

        let info = ActiveMeeting {
            id,
            title,
            started_at,
            microphone: microphone_name,
            system_audio,
            system_audio_error,
        };
        log::info!(
            "Meeting {id} started: microphone '{}', system audio {:?}",
            info.microphone,
            info.system_audio
        );
        *running = Some(Running {
            info: info.clone(),
            started: Instant::now(),
            tracks,
            writers,
            transcriber,
        });
        drop(running);
        Self::emit(&self.app, id);
        Ok(info)
    }

    /// Stops recording. Transcription of the last segments continues in the
    /// background; the meeting turns `done` when it finishes.
    pub fn stop(&self) -> Result<i64> {
        let Some(running) = self.lock().take() else {
            return Err(anyhow!("no meeting is being recorded"));
        };
        let id = running.info.id;
        let elapsed = running.started.elapsed().as_millis() as i64;
        for track in running.tracks {
            track.stop();
        }
        self.store
            .set_meeting_status(id, MeetingStatus::Transcribing, elapsed)?;
        Self::emit(&self.app, id);

        let app = self.app.clone();
        let store = self.store.clone();
        std::thread::spawn(move || {
            let mut length = 0;
            for writer in running.writers {
                match writer.join() {
                    Ok(Ok(ms)) => length = length.max(ms),
                    Ok(Err(e)) => log::error!("Meeting {id}: a track failed to save: {e:#}"),
                    Err(_) => log::error!("Meeting {id}: a track writer panicked"),
                }
            }
            // The writers dropped their segment senders, so the transcriber
            // finishes once the queued segments are done.
            let _ = running.transcriber.join();
            if let Err(e) = store.set_meeting_status(id, MeetingStatus::Done, length) {
                log::error!("Meeting {id}: could not mark it done: {e:#}");
            }
            log::info!("Meeting {id} finished ({} s)", length / 1000);
            if let Some(sync) = app.try_state::<Arc<crate::hikaya::sync_worker::SyncWorker>>() {
                sync.trigger();
            }
            if let Some(meetings) = app.try_state::<Arc<MeetingManager>>() {
                if let Err(e) = meetings.prune_audio() {
                    log::warn!("Could not remove old meeting audio: {e:#}");
                }
            }
            MeetingManager::emit(&app, id);
        });
        Ok(id)
    }

    /// Deletes a meeting, its transcript and its audio. Not the one recording.
    pub fn delete(&self, id: i64) -> Result<()> {
        if self.active().is_some_and(|a| a.id == id) {
            return Err(anyhow!("stop the meeting before deleting it"));
        }
        self.store.delete_meeting(id)?;
        let dir = self.audio_dir(id)?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        Ok(())
    }
}

/// Transcribes segments as they arrive, until both tracks have finished.
fn transcribe_segments(
    app: &AppHandle,
    store: &LearningStore,
    id: i64,
    segments: mpsc::Receiver<Segment>,
) {
    let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() else {
        log::error!("Meeting {id}: transcription is unavailable");
        return;
    };
    let control = GiveWayToDictation { app };
    for segment in segments {
        let length_ms = segment.samples.len() as i64 * 1000 / i64::from(SAMPLE_RATE);
        // Whisper makes up words for silence; quiet segments are skipped.
        if !hikaya_core::audio::has_speech(&segment.samples) {
            continue;
        }
        let text = match tm.transcribe_in_chunks(segment.samples, &control) {
            Ok(Some(t)) => t.text,
            Ok(None) => continue,
            Err(e) => {
                log::warn!(
                    "Meeting {id}: segment {} of {:?} ({}) not transcribed: {e:#}",
                    segment.index,
                    segment.speaker,
                    segment.path.display()
                );
                continue;
            }
        };
        let text = crate::hikaya::stage::apply(app, text);
        if text.trim().is_empty() {
            continue;
        }
        let line = TranscriptLine {
            start_ms: segment.start_ms,
            end_ms: segment.start_ms + length_ms,
            speaker: segment.speaker,
            text,
        };
        match store.add_meeting_line(id, &line) {
            Ok(()) => MeetingManager::emit(app, id),
            Err(e) => log::error!("Meeting {id}: could not save a line: {e:#}"),
        }
    }
}
