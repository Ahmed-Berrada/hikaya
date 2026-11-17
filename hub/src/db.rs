//! Schema of the encrypted hub database.
//!
//! Migrations are plain SQL run in order. The number of migrations already
//! applied is kept in SQLite's `user_version`.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// One entry per schema change, oldest first. Never edit or reorder an entry
/// that has shipped: add a new one.
const MIGRATIONS: &[&str] = &["CREATE TABLE devices (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        token_sha256 TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked INTEGER NOT NULL DEFAULT 0
    );",
    // Synced data. `seq` orders rule changes for the pull feed.
    "CREATE TABLE dictations (
        id TEXT PRIMARY KEY,
        device_id INTEGER NOT NULL REFERENCES devices(id),
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        text TEXT NOT NULL,
        language TEXT,
        duration_ms INTEGER,
        received_at INTEGER NOT NULL
    );
    CREATE INDEX dictations_created_at ON dictations (created_at DESC);
    CREATE VIRTUAL TABLE dictations_fts USING fts5(
        text, content='dictations', content_rowid='rowid', tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER dictations_ai AFTER INSERT ON dictations BEGIN
        INSERT INTO dictations_fts (rowid, text) VALUES (new.rowid, new.text);
    END;
    CREATE TRIGGER dictations_ad AFTER DELETE ON dictations BEGIN
        INSERT INTO dictations_fts (dictations_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    END;
    CREATE TRIGGER dictations_au AFTER UPDATE OF text ON dictations BEGIN
        INSERT INTO dictations_fts (dictations_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
        INSERT INTO dictations_fts (rowid, text) VALUES (new.rowid, new.text);
    END;
    CREATE TABLE rules (
        id TEXT PRIMARY KEY,
        device_id INTEGER NOT NULL REFERENCES devices(id),
        pattern TEXT NOT NULL,
        replacement TEXT NOT NULL,
        enabled INTEGER NOT NULL,
        source TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0,
        seq INTEGER NOT NULL
    );
    CREATE INDEX rules_seq ON rules (seq);
    CREATE TABLE substitutions (
        id TEXT PRIMARY KEY,
        device_id INTEGER NOT NULL REFERENCES devices(id),
        from_text TEXT NOT NULL,
        to_text TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sync_counter (id INTEGER PRIMARY KEY CHECK (id = 1), seq INTEGER NOT NULL);
    INSERT INTO sync_counter (id, seq) VALUES (1, 0);",
    // Meetings: the transcript is stored as JSON lines plus a plain-text copy
    // for search.
    "CREATE TABLE meetings (
        id TEXT PRIMARY KEY,
        device_id INTEGER NOT NULL REFERENCES devices(id),
        title TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        summary TEXT,
        lines_json TEXT NOT NULL,
        transcript TEXT NOT NULL,
        line_count INTEGER NOT NULL,
        received_at INTEGER NOT NULL
    );
    CREATE INDEX meetings_started_at ON meetings (started_at DESC);
    CREATE VIRTUAL TABLE meetings_fts USING fts5(
        title, summary, transcript, content='meetings', content_rowid='rowid',
        tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER meetings_ai AFTER INSERT ON meetings BEGIN
        INSERT INTO meetings_fts (rowid, title, summary, transcript)
        VALUES (new.rowid, new.title, coalesce(new.summary, ''), new.transcript);
    END;
    CREATE TRIGGER meetings_ad AFTER DELETE ON meetings BEGIN
        INSERT INTO meetings_fts (meetings_fts, rowid, title, summary, transcript)
        VALUES ('delete', old.rowid, old.title, coalesce(old.summary, ''), old.transcript);
    END;
    CREATE TRIGGER meetings_au AFTER UPDATE ON meetings BEGIN
        INSERT INTO meetings_fts (meetings_fts, rowid, title, summary, transcript)
        VALUES ('delete', old.rowid, old.title, coalesce(old.summary, ''), old.transcript);
        INSERT INTO meetings_fts (rowid, title, summary, transcript)
        VALUES (new.rowid, new.title, coalesce(new.summary, ''), new.transcript);
    END;",
    // Recordings made on a phone, waiting for a computer (ADR 0004). The
    // audio is kept as chunks until the recording becomes a meeting.
    "CREATE TABLE recordings (
        id TEXT PRIMARY KEY,
        device_id INTEGER NOT NULL REFERENCES devices(id),
        title TEXT NOT NULL,
        mime TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        bytes INTEGER NOT NULL DEFAULT 0,
        claimed_by INTEGER REFERENCES devices(id),
        claimed_at INTEGER,
        attempts INTEGER NOT NULL DEFAULT 0,
        error TEXT,
        meeting_id TEXT
    );
    CREATE INDEX recordings_waiting ON recordings (status, created_at);
    CREATE TABLE recording_chunks (
        recording_id TEXT NOT NULL REFERENCES recordings(id),
        idx INTEGER NOT NULL,
        data BLOB NOT NULL,
        PRIMARY KEY (recording_id, idx)
    );",
];

/// Brings the database to the current schema. Each migration and its version
/// bump commit together, so a crash never leaves a half-applied step.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let applied: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .context("reading the schema version")?
        .try_into()
        .context("the schema version is negative")?;
    anyhow::ensure!(
        applied <= MIGRATIONS.len(),
        "the database is at schema {applied}, newer than this hub ({}); upgrade the hub",
        MIGRATIONS.len()
    );
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying migration {}", index + 1))?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version")
    }

    #[test]
    fn migrates_a_new_database_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().expect("open");
        migrate(&mut conn).expect("first run");
        assert_eq!(version(&conn), MIGRATIONS.len() as i64);
        migrate(&mut conn).expect("second run changes nothing");
        assert_eq!(version(&conn), MIGRATIONS.len() as i64);
        conn.execute(
            "INSERT INTO devices (name, token_sha256, created_at) VALUES ('a', 'b', 0)",
            [],
        )
        .expect("the devices table exists");
    }

    #[test]
    fn refuses_a_database_from_a_newer_hub() {
        let mut conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1)
            .expect("set version");
        let error = migrate(&mut conn).expect_err("must refuse");
        assert!(error.to_string().contains("upgrade the hub"));
    }
}
