//! Tauri commands for meetings. Names start with `hikaya_meeting`.

use super::manager::{ActiveMeeting, MeetingManager};
use super::store::{MeetingDetail, MeetingSummary};
use super::summary::{summarize, SummaryModel};
use crate::hikaya::store::LearningStore;
use hikaya_core::meeting::{to_markdown, to_srt, MeetingExport, SpeakerNames};
use serde::Deserialize;
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, State};

type Meetings<'a> = State<'a, Arc<MeetingManager>>;
type Store<'a> = State<'a, Arc<LearningStore>>;

fn text_error(err: anyhow::Error) -> String {
    format!("{err:#}")
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_start(
    meetings: Meetings<'_>,
    title: String,
) -> Result<ActiveMeeting, String> {
    meetings.start(&title).map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_stop(meetings: Meetings<'_>) -> Result<i64, String> {
    meetings.stop().map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_active(meetings: Meetings<'_>) -> Option<ActiveMeeting> {
    meetings.active()
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meetings_list(store: Store<'_>) -> Result<Vec<MeetingSummary>, String> {
    store.list_meetings().map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_get(store: Store<'_>, id: i64) -> Result<Option<MeetingDetail>, String> {
    store.meeting_detail(id).map_err(text_error)
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_rename(
    app: AppHandle,
    store: Store<'_>,
    id: i64,
    title: String,
) -> Result<(), String> {
    store.rename_meeting(id, &title).map_err(text_error)?;
    sync_soon(&app);
    Ok(())
}

/// Sends the change to the hub right away, if one is connected.
fn sync_soon(app: &AppHandle) {
    use tauri::Manager;
    if let Some(sync) = app.try_state::<Arc<crate::hikaya::sync_worker::SyncWorker>>() {
        sync.trigger();
    }
}

#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_delete(meetings: Meetings<'_>, id: i64) -> Result<(), String> {
    meetings.delete(id).map_err(text_error)
}

/// Summarizes a meeting with the post-processing provider and saves it.
#[tauri::command]
#[specta::specta]
pub async fn hikaya_meeting_summarize(
    app: AppHandle,
    store: Store<'_>,
    id: i64,
    you: String,
    others: String,
) -> Result<String, String> {
    let model = SummaryModel::from_settings(&crate::settings::get_settings(&app))?;
    let lines = store.meeting_lines(id).map_err(text_error)?;
    let summary = summarize(&model, &lines, &SpeakerNames { you, others }).await?;
    store
        .set_meeting_summary(id, &summary, model.name())
        .map_err(text_error)?;
    sync_soon(&app);
    Ok(summary)
}

/// Words the export uses, in the UI's language.
#[derive(Debug, Deserialize, Type)]
pub struct ExportLabels {
    pub you: String,
    pub others: String,
    pub summary: String,
    pub transcript: String,
    /// The meeting's start, already formatted for the user.
    pub started: String,
}

/// File formats a meeting can be exported to.
#[derive(Debug, Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Markdown,
    Srt,
}

/// The meeting as Markdown or SRT text.
#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_export(
    store: Store<'_>,
    id: i64,
    format: ExportFormat,
    labels: ExportLabels,
) -> Result<String, String> {
    let detail = store
        .meeting_detail(id)
        .map_err(text_error)?
        .ok_or("no such meeting")?;
    let lines = store.meeting_lines(id).map_err(text_error)?;
    let names = SpeakerNames {
        you: labels.you,
        others: labels.others,
    };
    Ok(match format {
        ExportFormat::Markdown => to_markdown(&MeetingExport {
            title: &detail.meeting.title,
            started: &labels.started,
            duration_ms: detail.meeting.duration_ms,
            summary: detail.summary.as_deref(),
            lines: &lines,
            names: &names,
            summary_heading: &labels.summary,
            transcript_heading: &labels.transcript,
        }),
        ExportFormat::Srt => to_srt(&lines, &names),
    })
}

/// Writes exported text to a file the user picked in a save dialog.
#[tauri::command]
#[specta::specta]
pub fn hikaya_meeting_save_export(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("could not write {path}: {e}"))
}
