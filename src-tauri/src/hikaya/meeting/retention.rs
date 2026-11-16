//! Deletes old meeting audio, following the recording-retention setting that
//! already governs dictation recordings. Transcripts and summaries are kept:
//! they are small, and they are what the archive is for.

use super::manager::MeetingManager;
use crate::settings::RecordingRetentionPeriod;
use anyhow::Result;

/// How long meeting audio is kept, or `None` to keep it. "Keep the last N"
/// counts dictations, so it does not apply to meetings.
fn max_age_ms(period: RecordingRetentionPeriod) -> Option<i64> {
    const DAY: i64 = 24 * 60 * 60 * 1000;
    match period {
        RecordingRetentionPeriod::Never | RecordingRetentionPeriod::PreserveLimit => None,
        RecordingRetentionPeriod::Days3 => Some(3 * DAY),
        RecordingRetentionPeriod::Weeks2 => Some(14 * DAY),
        RecordingRetentionPeriod::Months3 => Some(90 * DAY),
    }
}

/// Meetings whose audio should go: older than the limit, and not the one
/// being recorded. `meetings` is `(id, started_at_ms)`.
pub fn expired(
    meetings: &[(i64, i64)],
    active: Option<i64>,
    now_ms: i64,
    period: RecordingRetentionPeriod,
) -> Vec<i64> {
    let Some(max_age) = max_age_ms(period) else {
        return Vec::new();
    };
    meetings
        .iter()
        .filter(|(id, started_at)| Some(*id) != active && now_ms - started_at > max_age)
        .map(|(id, _)| *id)
        .collect()
}

impl MeetingManager {
    /// Removes the audio of meetings past the retention period. Returns how
    /// many meetings lost their audio.
    pub fn prune_audio(&self) -> Result<usize> {
        let period = crate::settings::get_recording_retention_period(self.app());
        let meetings: Vec<(i64, i64)> = self
            .store()
            .list_meetings()?
            .into_iter()
            .filter(|m| {
                matches!(
                    m.status,
                    super::store::MeetingStatus::Done | super::store::MeetingStatus::Interrupted
                )
            })
            .map(|m| (m.id, m.started_at))
            .collect();
        let active = self.active().map(|a| a.id);
        let now = chrono::Utc::now().timestamp_millis();
        let mut removed = 0;
        for id in expired(&meetings, active, now, period) {
            let dir = self.audio_dir(id)?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                removed += 1;
            }
        }
        if removed > 0 {
            log::info!("Removed the audio of {removed} meeting(s) past the retention period");
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn keeps_everything_for_never_and_count_based_retention() {
        let meetings = [(1, 0), (2, 0)];
        for period in [
            RecordingRetentionPeriod::Never,
            RecordingRetentionPeriod::PreserveLimit,
        ] {
            assert!(expired(&meetings, None, 1_000 * DAY, period).is_empty());
        }
    }

    #[test]
    fn removes_only_what_is_older_than_the_period() {
        let now = 100 * DAY;
        let meetings = [
            (1, now - 4 * DAY),
            (2, now - 2 * DAY),
            (3, now - 20 * DAY),
            (4, now - 100 * DAY),
        ];
        assert_eq!(
            expired(&meetings, None, now, RecordingRetentionPeriod::Days3),
            [1, 3, 4]
        );
        assert_eq!(
            expired(&meetings, None, now, RecordingRetentionPeriod::Weeks2),
            [3, 4]
        );
        assert_eq!(
            expired(&meetings, None, now, RecordingRetentionPeriod::Months3),
            [4]
        );
    }

    #[test]
    fn never_touches_the_meeting_being_recorded() {
        let now = 100 * DAY;
        assert!(expired(&[(7, 0)], Some(7), now, RecordingRetentionPeriod::Days3).is_empty());
    }
}
