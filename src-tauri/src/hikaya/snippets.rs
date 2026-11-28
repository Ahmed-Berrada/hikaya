//! Snippets on this device: saved texts typed by a spoken cue. Stored in
//! Hikaya's database and synced through the hub like rules (see `sync`).

use super::store::{now_ms, LearningStore};
use anyhow::{Context, Result};
use hikaya_core::snippets::{normalize_cue, valid, Snippet, MAX_CUE_BYTES, MAX_TEXT_BYTES};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;

/// A snippet as stored and as shown in the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StoredSnippet {
    /// Chosen when it was created, the same on every device.
    pub id: String,
    /// What the user says.
    pub cue: String,
    /// What gets typed.
    pub text: String,
}

impl LearningStore {
    /// Every snippet except deleted ones, by cue.
    pub fn list_snippets(&self) -> Result<Vec<StoredSnippet>> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare("SELECT uid, cue, text FROM snippets WHERE deleted = 0 ORDER BY lower(cue)")?;
        let snippets = statement
            .query_map([], |row| {
                Ok(StoredSnippet {
                    id: row.get(0)?,
                    cue: row.get(1)?,
                    text: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(snippets)
    }

    /// The snippets the dictation pipeline expands.
    pub fn active_snippets(&self) -> Vec<Snippet> {
        self.list_snippets()
            .map(|list| {
                list.into_iter()
                    .map(|s| Snippet {
                        cue: s.cue,
                        text: s.text,
                    })
                    .collect()
            })
            .unwrap_or_else(|e| {
                log::warn!("Could not read the snippets: {e:#}");
                Vec::new()
            })
    }

    /// Saves a snippet. A cue already in use (same words, ignoring case and
    /// accents) gets the new text, so one cue never types two things.
    pub fn save_snippet(&self, cue: &str, text: &str) -> Result<StoredSnippet> {
        let cue = cue.trim();
        // Only the end: indentation and blank lines at the start are the user's.
        let text = text.trim_end();
        anyhow::ensure!(
            valid(cue, text),
            "a snippet needs a cue (at most {MAX_CUE_BYTES} bytes) and a text (at most {MAX_TEXT_BYTES})"
        );
        let key = normalize_cue(cue);
        let conn = self.conn()?;
        let existing = conn
            .prepare("SELECT uid, cue FROM snippets ORDER BY deleted, updated_at DESC")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .find(|(_, other)| normalize_cue(other) == key)
            .map(|(uid, _)| uid);
        let now = now_ms();
        let uid = match existing {
            Some(uid) => {
                conn.execute(
                    // max(): the change must beat the version this device saw,
                    // even when this computer's clock is behind.
                    "UPDATE snippets SET cue = ?1, text = ?2, deleted = 0,
                       updated_at = max(?3, updated_at + 1), synced = 0
                     WHERE uid = ?4",
                    params![cue, text, now, uid],
                )?;
                uid
            }
            None => conn.query_row(
                "INSERT INTO snippets (uid, cue, text, updated_at)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3) RETURNING uid",
                params![cue, text, now],
                |row| row.get(0),
            )?,
        };
        let saved = conn
            .query_row(
                "SELECT uid, cue, text FROM snippets WHERE uid = ?1",
                params![uid],
                |row| {
                    Ok(StoredSnippet {
                        id: row.get(0)?,
                        cue: row.get(1)?,
                        text: row.get(2)?,
                    })
                },
            )
            .optional()?;
        saved.context("the snippet that was just saved is missing")
    }

    /// Deletes a snippet here and, on the next sync, on the user's other
    /// devices.
    pub fn delete_snippet(&self, id: &str) -> Result<()> {
        self.conn()?.execute(
            // max(): see save_snippet.
            "UPDATE snippets SET deleted = 1, updated_at = max(?1, updated_at + 1), synced = 0
             WHERE uid = ?2",
            params![now_ms(), id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open");
        (dir, store)
    }

    #[test]
    fn saving_the_same_cue_again_replaces_its_text() {
        let (_dir, store) = store();
        let first = store
            .save_snippet("My address", "12 rue de la Paix\n75002 Paris")
            .expect("save");
        let second = store
            .save_snippet(" my ADDRESS ", "Elsewhere")
            .expect("save");
        assert_eq!(first.id, second.id);
        let list = store.list_snippets().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].text, "Elsewhere");
        assert_eq!(store.active_snippets()[0].cue, "my ADDRESS");
    }

    #[test]
    fn a_deleted_snippet_stops_and_its_cue_can_be_reused() {
        let (_dir, store) = store();
        let saved = store.save_snippet("sig", "— Ahmed").expect("save");
        store.delete_snippet(&saved.id).expect("delete");
        assert!(store.active_snippets().is_empty());
        let again = store.save_snippet("sig", "Ahmed B.").expect("save");
        assert_eq!(again.id, saved.id);
        assert_eq!(store.list_snippets().expect("list").len(), 1);
    }

    #[test]
    fn invalid_snippets_are_refused() {
        let (_dir, store) = store();
        assert!(store.save_snippet("   ", "text").is_err());
        assert!(store.save_snippet("cue", "  \n ").is_err());
        assert!(store.list_snippets().expect("list").is_empty());
    }
}
