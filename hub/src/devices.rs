//! Devices allowed to talk to the hub. Each one has a random token; only a
//! SHA-256 of the token is stored, so a stolen database backup holds no
//! usable tokens (and the database is encrypted anyway).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// A registered device, without its token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Device {
    pub id: i64,
    pub name: String,
    /// Unix time in seconds.
    pub created_at: i64,
    /// Unix time of the last authenticated request, if any.
    pub last_seen_at: Option<i64>,
    pub revoked: bool,
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Registers a device and returns it with its token. The token is shown
/// once and cannot be recovered later.
pub fn register(conn: &Connection, name: &str) -> Result<(Device, String)> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "a device needs a name");
    anyhow::ensure!(name.chars().count() <= 64, "the device name is too long");
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("hky_{}", B64URL.encode(bytes));
    conn.execute(
        "INSERT INTO devices (name, token_sha256, created_at) VALUES (?1, ?2, strftime('%s','now'))",
        params![name, hash(&token)],
    )
    .context("saving the device")?;
    let device = get(conn, conn.last_insert_rowid())?.context("the new device is missing")?;
    Ok((device, token))
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        last_seen_at: row.get(3)?,
        revoked: row.get(4)?,
    })
}

const COLUMNS: &str = "id, name, created_at, last_seen_at, revoked";

fn get(conn: &Connection, id: i64) -> Result<Option<Device>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM devices WHERE id = ?1"),
            params![id],
            from_row,
        )
        .optional()?)
}

/// Every device, oldest first.
pub fn list(conn: &Connection) -> Result<Vec<Device>> {
    let mut statement = conn.prepare(&format!("SELECT {COLUMNS} FROM devices ORDER BY id"))?;
    let devices = statement
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(devices)
}

/// The device a token belongs to, if the token is valid and not revoked.
/// Also records that the device was just seen.
pub fn authenticate(conn: &Connection, token: &str) -> Result<Option<Device>> {
    // The lookup is by hash, so the comparison does not leak the token by timing.
    let device = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM devices WHERE token_sha256 = ?1 AND revoked = 0"),
            params![hash(token)],
            from_row,
        )
        .optional()?;
    if let Some(device) = &device {
        conn.execute(
            "UPDATE devices SET last_seen_at = strftime('%s','now') WHERE id = ?1",
            params![device.id],
        )?;
    }
    Ok(device)
}

/// Revokes a device. Returns false if there is no such device.
pub fn revoke(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("UPDATE devices SET revoked = 1 WHERE id = ?1", params![id])? > 0)
}
