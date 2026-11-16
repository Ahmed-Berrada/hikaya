//! Schema of the encrypted hub database.

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

const MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE devices (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        token_sha256 TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked INTEGER NOT NULL DEFAULT 0
    );",
)];

/// Brings the database to the current schema.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    Migrations::new(MIGRATIONS.to_vec())
        .to_latest(conn)
        .context("migrating the hub database")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        Migrations::new(MIGRATIONS.to_vec())
            .validate()
            .expect("valid migrations");
    }
}
