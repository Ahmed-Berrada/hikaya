//! Usage numbers for the Learning screen: words dictated, an estimate of the
//! typing time saved, and what the user corrects most.

use super::store::LearningStore;
use anyhow::Result;
use rusqlite::params;
use serde::Serialize;
use specta::Type;

/// Typing speed used for the estimate, in words per minute (an average
/// typist). Speaking is counted at about 150; the saving is the difference.
const TYPING_WPM: f64 = 40.0;
const SPEAKING_WPM: f64 = 150.0;
/// Corrections listed in the panel.
const TOP_CORRECTIONS: i64 = 5;

/// One correction and how often the user made it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct CorrectionCount {
    pub from: String,
    pub to: String,
    pub count: u32,
}

/// Everything the stats panel shows.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct Stats {
    pub words_today: u32,
    pub words_week: u32,
    pub words_total: u32,
    pub dictations_total: u32,
    /// Estimated minutes saved compared with typing, since counting began.
    pub minutes_saved: u32,
    pub rules: u32,
    pub learned_rules: u32,
    pub top_corrections: Vec<CorrectionCount>,
}

/// Words in a dictation: whitespace-separated pieces that contain a letter
/// or digit, so "v0.3.1" and "l'équipe" count once and "—" not at all.
pub fn count_words(text: &str) -> u32 {
    let n = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn minutes_saved(words: u32) -> u32 {
    let words = f64::from(words);
    (words / TYPING_WPM - words / SPEAKING_WPM).max(0.0).round() as u32
}

impl LearningStore {
    /// Counts one dictation of `text` on the local `day` (`YYYY-MM-DD`).
    pub fn count_dictation(&self, day: &str, text: &str) -> Result<()> {
        let words = count_words(text);
        if words == 0 {
            return Ok(());
        }
        self.conn()?.execute(
            "INSERT INTO daily_stats (day, dictations, words) VALUES (?1, 1, ?2)
             ON CONFLICT (day) DO UPDATE SET dictations = dictations + 1, words = words + ?2",
            params![day, words],
        )?;
        Ok(())
    }

    /// The numbers for the panel. `today` and `week_start` are local days
    /// (`YYYY-MM-DD`); the week runs from `week_start` to `today`.
    pub fn stats(&self, today: &str, week_start: &str) -> Result<Stats> {
        let conn = self.conn()?;
        let sum = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<u32> {
            let n: i64 = conn.query_row(sql, args, |row| row.get(0))?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        };
        let words_today = sum(
            "SELECT coalesce(sum(words), 0) FROM daily_stats WHERE day = ?1",
            &[&today],
        )?;
        let words_week = sum(
            "SELECT coalesce(sum(words), 0) FROM daily_stats WHERE day >= ?1 AND day <= ?2",
            &[&week_start, &today],
        )?;
        let words_total = sum("SELECT coalesce(sum(words), 0) FROM daily_stats", &[])?;
        let dictations_total = sum("SELECT coalesce(sum(dictations), 0) FROM daily_stats", &[])?;
        let rules = sum("SELECT count(*) FROM rules WHERE deleted = 0", &[])?;
        let learned_rules = sum(
            "SELECT count(*) FROM rules WHERE deleted = 0 AND source = 'learned'",
            &[],
        )?;
        let top_corrections = conn
            .prepare(
                "SELECT from_text, to_text, count(*) AS n FROM substitutions
                 GROUP BY lower(from_text), to_text ORDER BY n DESC, max(id) DESC LIMIT ?1",
            )?
            .query_map(params![TOP_CORRECTIONS], |row| {
                Ok(CorrectionCount {
                    from: row.get(0)?,
                    to: row.get(1)?,
                    count: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Stats {
            words_today,
            words_week,
            words_total,
            dictations_total,
            minutes_saved: minutes_saved(words_total),
            rules,
            learned_rules,
            top_corrections,
        })
    }
}

/// Today and the first day of the last seven, in local time.
pub fn local_week() -> (String, String) {
    let today = chrono::Local::now().date_naive();
    let start = today - chrono::Duration::days(6);
    (
        today.format("%Y-%m-%d").to_string(),
        start.format("%Y-%m-%d").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open");
        (dir, store)
    }

    #[test]
    fn words_are_counted_in_both_languages() {
        assert_eq!(count_words("Bonjour, c'est l'équipe produit !"), 4);
        assert_eq!(count_words("Push the fix to main — then ship v0.3.1."), 8);
        assert_eq!(count_words("  ...  "), 0);
        assert_eq!(count_words("e-mail à 14h30"), 3);
    }

    #[test]
    fn days_add_up_into_today_week_and_total() {
        let (_dir, store) = store();
        store
            .count_dictation("2026-11-01", "one two three")
            .expect("count");
        store
            .count_dictation("2026-11-14", "un deux")
            .expect("count");
        store
            .count_dictation("2026-11-16", "a b c d")
            .expect("count");
        store.count_dictation("2026-11-16", "e f").expect("count");
        store
            .count_dictation("2026-11-16", "   ")
            .expect("blank is skipped");
        let stats = store.stats("2026-11-16", "2026-11-10").expect("stats");
        assert_eq!(stats.words_today, 6);
        assert_eq!(stats.words_week, 8);
        assert_eq!(stats.words_total, 11);
        assert_eq!(stats.dictations_total, 4);
    }

    #[test]
    fn time_saved_is_typing_time_minus_speaking_time() {
        assert_eq!(minutes_saved(0), 0);
        // 6,000 words: 150 min typing, 40 min speaking.
        assert_eq!(minutes_saved(6_000), 110);
    }

    #[test]
    fn top_corrections_and_rule_counts() {
        let (_dir, store) = store();
        for _ in 0..3 {
            store
                .record_correction(None, "push to get hub", "push to GitHub")
                .expect("record");
        }
        store
            .record_correction(None, "ask a med", "ask Ahmed")
            .expect("record");
        store.add_rule("clod", "Claude", "manual").expect("rule");
        store.accept_proposal("get hub", "GitHub").expect("learned");
        let stats = store.stats("2026-11-16", "2026-11-10").expect("stats");
        assert_eq!(
            stats.top_corrections[0],
            CorrectionCount {
                from: "get hub".into(),
                to: "GitHub".into(),
                count: 3
            }
        );
        assert_eq!(stats.top_corrections[1].to, "Ahmed");
        assert_eq!((stats.rules, stats.learned_rules), (2, 1));
    }

    #[test]
    fn local_week_spans_seven_days() {
        let (today, start) = local_week();
        let t = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").expect("today");
        let s = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").expect("start");
        assert_eq!((t - s).num_days(), 6);
    }
}
