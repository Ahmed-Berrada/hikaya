//! Consistent copies of the encrypted database, for backups.
//!
//! Copying `hub.db` while the hub writes to it can capture a half-written
//! state. A snapshot is made with `VACUUM INTO`, which SQLCipher writes with
//! the same key, so snapshots are as unreadable as the database itself.
//! Backup tools should copy the `snapshots/` directory.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Snapshots kept; older ones are deleted.
pub const KEEP: usize = 3;
const PREFIX: &str = "hub-";
const SUFFIX: &str = ".db";

/// A UTC time as `YYYYMMDDTHHMMSS`, which sorts in time order.
pub fn stamp(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Writes a snapshot into `dir` and deletes all but the newest [`KEEP`].
/// Returns the new snapshot's path.
pub fn take(conn: &Connection, dir: &Path, stamp: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
    let partial = dir.join(format!("{PREFIX}{stamp}{SUFFIX}.partial"));
    let _ = std::fs::remove_file(&partial);
    let target = partial
        .to_str()
        .context("the snapshot path is not valid UTF-8")?;
    conn.execute("VACUUM INTO ?1", [target])
        .context("writing the snapshot")?;
    // Rename only once complete, so a crash never leaves a broken snapshot
    // that looks like a good one.
    std::fs::rename(&partial, &path).context("finishing the snapshot")?;

    let mut existing: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        })
        .collect();
    existing.sort();
    let excess = existing.len().saturating_sub(KEEP);
    for old in existing.into_iter().take(excess) {
        std::fs::remove_file(&old).with_context(|| format!("removing {}", old.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn stamps_known_dates() {
        let at = |secs: u64| stamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "19700101T000000");
        assert_eq!(at(951_782_400), "20000229T000000"); // leap day
        assert_eq!(at(1_794_829_303), "20261116T114143"); // matches the hub log of that request
        assert_eq!(at(4_102_444_799), "20991231T235959");
    }
    use crate::vault::{test_cost, Vault};

    #[test]
    fn a_snapshot_is_encrypted_with_the_same_key_and_old_ones_are_pruned() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::with_cost(dir.path(), test_cost());
        let key = vault
            .create("correct horse battery staple")
            .expect("create");
        let conn = vault.connect(&key).expect("connect");
        conn.execute_batch(
            "CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('note confidentielle');",
        )
        .expect("write");

        let snapshots = dir.path().join("snapshots");
        for stamp in [
            "20261101T000000",
            "20261102T000000",
            "20261103T000000",
            "20261104T000000",
        ] {
            take(&conn, &snapshots, stamp).expect("snapshot");
        }
        let mut names: Vec<String> = std::fs::read_dir(&snapshots)
            .expect("list")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf8"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "hub-20261102T000000.db",
                "hub-20261103T000000.db",
                "hub-20261104T000000.db"
            ]
        );

        let latest = snapshots.join("hub-20261104T000000.db");
        let bytes = std::fs::read(&latest).expect("read");
        assert!(!bytes
            .windows(12)
            .any(|w| w == b"confidential" || w == b"confidentiel"));
        assert!(!bytes.starts_with(b"SQLite format 3"));

        // The same key opens it, through the same code path as the live database.
        let restored = Vault::with_cost(dir.path(), test_cost());
        let copy = std::fs::read(&latest).expect("read");
        std::fs::write(dir.path().join("hub.db"), copy).expect("restore");
        let key = restored
            .unlock("correct horse battery staple")
            .expect("unlock restored");
        let text: String = restored
            .connect(&key)
            .expect("connect")
            .query_row("SELECT text FROM notes", [], |r| r.get(0))
            .expect("read restored");
        assert_eq!(text, "note confidentielle");
    }
}
