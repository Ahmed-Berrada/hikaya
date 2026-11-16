//! The vault: the hub's data, encrypted with a key derived from a passphrase.
//!
//! On disk there are two files. `vault.json` holds the salt and the key
//! derivation settings (nothing secret). `hub.db` is a SQLCipher database
//! that only opens with the derived key. The key itself is never written
//! anywhere: after a restart the hub is locked until someone sends the
//! passphrase again.

use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Shortest passphrase the hub accepts when a vault is created.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

const VAULT_FILE: &str = "vault.json";
const DB_FILE: &str = "hub.db";

/// What is stored in `vault.json`. None of it is secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultHeader {
    /// Format version of this file.
    pub version: u32,
    /// Argon2id memory cost in KiB.
    pub memory_kib: u32,
    /// Argon2id passes.
    pub iterations: u32,
    /// Argon2id lanes.
    pub parallelism: u32,
    /// Random salt, base64.
    pub salt: String,
}

/// Key derivation cost. The defaults follow the OWASP Argon2id guidance;
/// tests use a cheap setting so they run fast.
#[derive(Clone, Copy, Debug)]
pub struct KdfCost {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfCost {
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// The 32-byte database key. Wiped from memory when dropped.
pub type VaultKey = Zeroizing<[u8; 32]>;

/// Where the vault lives and how to open it.
#[derive(Clone, Debug)]
pub struct Vault {
    dir: PathBuf,
    cost: KdfCost,
}

/// Why opening the vault failed.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("the passphrase is wrong")]
    WrongPassphrase,
    #[error("the vault has not been created yet")]
    NotInitialized,
    #[error("the vault already exists")]
    AlreadyInitialized,
    #[error("the passphrase must be at least {MIN_PASSPHRASE_CHARS} characters")]
    WeakPassphrase,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Vault {
    /// A vault stored in `dir` with the default key derivation cost.
    pub fn new(dir: &Path) -> Self {
        Self::with_cost(dir, KdfCost::default())
    }

    /// A vault stored in `dir` with a chosen key derivation cost.
    pub fn with_cost(dir: &Path, cost: KdfCost) -> Self {
        Self {
            dir: dir.to_path_buf(),
            cost,
        }
    }

    fn header_path(&self) -> PathBuf {
        self.dir.join(VAULT_FILE)
    }

    /// Path of the encrypted database.
    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE)
    }

    /// True once a vault has been created in this directory.
    pub fn is_initialized(&self) -> bool {
        self.header_path().is_file()
    }

    /// Creates the vault and returns its key.
    pub fn create(&self, passphrase: &str) -> Result<VaultKey, VaultError> {
        if self.is_initialized() {
            return Err(VaultError::AlreadyInitialized);
        }
        if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
            return Err(VaultError::WeakPassphrase);
        }
        std::fs::create_dir_all(&self.dir).context("creating the data directory")?;
        restrict_to_owner(&self.dir, 0o700)?;
        if self.db_path().exists() {
            return Err(VaultError::Other(anyhow::anyhow!(
                "{} exists without {VAULT_FILE}; refusing to overwrite it",
                self.db_path().display()
            )));
        }

        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let header = VaultHeader {
            version: 1,
            memory_kib: self.cost.memory_kib,
            iterations: self.cost.iterations,
            parallelism: self.cost.parallelism,
            salt: B64.encode(salt),
        };
        let key = derive_key(passphrase, &header)?;

        // Create the database first: if this fails no header is left behind.
        let conn = open_encrypted(&self.db_path(), &key)?;
        conn.execute_batch("CREATE TABLE vault_check (created_at INTEGER NOT NULL); INSERT INTO vault_check VALUES (strftime('%s','now'));")
            .context("initializing the encrypted database")?;
        drop(conn);
        restrict_to_owner(&self.db_path(), 0o600)?;

        let json = serde_json::to_vec_pretty(&header).context("encoding the vault header")?;
        write_private(&self.header_path(), &json)?;
        Ok(key)
    }

    /// Derives the key from the passphrase and checks it opens the database.
    pub fn unlock(&self, passphrase: &str) -> Result<VaultKey, VaultError> {
        if !self.is_initialized() {
            return Err(VaultError::NotInitialized);
        }
        let raw = std::fs::read(self.header_path()).context("reading the vault header")?;
        let header: VaultHeader =
            serde_json::from_slice(&raw).context("the vault header is not valid JSON")?;
        if header.version != 1 {
            return Err(VaultError::Other(anyhow::anyhow!(
                "vault format {} is not supported by this version",
                header.version
            )));
        }
        let key = derive_key(passphrase, &header)?;
        let conn = open_encrypted(&self.db_path(), &key)?;
        // With the wrong key SQLCipher cannot read the first page.
        match conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        }) {
            Ok(_) => Ok(key),
            Err(_) => Err(VaultError::WrongPassphrase),
        }
    }

    /// Opens a connection to the encrypted database with a key from
    /// [`Vault::create`] or [`Vault::unlock`].
    pub fn connect(&self, key: &VaultKey) -> Result<Connection> {
        open_encrypted(&self.db_path(), key)
    }
}

fn derive_key(passphrase: &str, header: &VaultHeader) -> Result<VaultKey> {
    let salt = B64
        .decode(&header.salt)
        .context("the vault salt is not valid base64")?;
    let params = Params::new(
        header.memory_kib,
        header.iterations,
        header.parallelism,
        Some(32),
    )
    .map_err(|e| anyhow::anyhow!("invalid key derivation settings: {e}"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("deriving the key failed: {e}"))?;
    Ok(key)
}

fn open_encrypted(path: &Path, key: &VaultKey) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    let hex: Zeroizing<String> = Zeroizing::new(key.iter().map(|b| format!("{b:02x}")).collect());
    // A raw key: SQLCipher uses it as is, with no second key derivation.
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex.as_str()))
        .context("setting the database key")?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
        .or_else(|e| {
            // With a wrong key the first real statement fails; let the caller's
            // own check report that as a wrong passphrase.
            if e.to_string().contains("not a database") {
                Ok(())
            } else {
                Err(e)
            }
        })
        .context("configuring the database")?;
    Ok(conn)
}

/// Removes group and world access. The data is encrypted, but file names,
/// sizes and timestamps are nobody else's business either.
fn restrict_to_owner(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("restricting access to {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Writes a file readable only by its owner.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_cost() -> KdfCost {
    KdfCost {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSPHRASE: &str = "correct horse battery staple";

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::with_cost(dir.path(), test_cost());
        (dir, vault)
    }

    #[test]
    fn a_new_directory_is_not_initialized() {
        let (_dir, vault) = vault();
        assert!(!vault.is_initialized());
        assert!(matches!(
            vault.unlock(PASSPHRASE),
            Err(VaultError::NotInitialized)
        ));
    }

    #[test]
    fn create_then_unlock_gives_the_same_key() {
        let (_dir, vault) = vault();
        let created = vault.create(PASSPHRASE).expect("create");
        assert!(vault.is_initialized());
        let unlocked = vault.unlock(PASSPHRASE).expect("unlock");
        assert_eq!(*created, *unlocked);
    }

    #[test]
    fn the_wrong_passphrase_is_rejected() {
        let (_dir, vault) = vault();
        vault.create(PASSPHRASE).expect("create");
        assert!(matches!(
            vault.unlock("correct horse battery stapler"),
            Err(VaultError::WrongPassphrase)
        ));
    }

    #[test]
    fn a_short_passphrase_is_refused_and_nothing_is_written() {
        let (dir, vault) = vault();
        assert!(matches!(
            vault.create("short"),
            Err(VaultError::WeakPassphrase)
        ));
        assert!(!vault.is_initialized());
        assert!(!dir.path().join(DB_FILE).exists());
    }

    #[test]
    fn a_vault_cannot_be_created_twice() {
        let (_dir, vault) = vault();
        vault.create(PASSPHRASE).expect("create");
        assert!(matches!(
            vault.create(PASSPHRASE),
            Err(VaultError::AlreadyInitialized)
        ));
    }

    #[test]
    fn data_survives_and_is_unreadable_without_the_key() {
        let (dir, vault) = vault();
        let key = vault.create(PASSPHRASE).expect("create");
        let conn = vault.connect(&key).expect("connect");
        conn.execute_batch(
            "CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('réunion secrète avec Ahmed');",
        )
        .expect("write");
        drop(conn);

        // The words are not in the file in the clear.
        let bytes = std::fs::read(dir.path().join(DB_FILE)).expect("read db file");
        let needle = "secr".as_bytes();
        assert!(!bytes.windows(needle.len()).any(|w| w == needle));
        assert!(!bytes.starts_with(b"SQLite format 3"));

        // A plain SQLite connection cannot read it.
        let plain = Connection::open(dir.path().join(DB_FILE)).expect("open plain");
        assert!(plain
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r
                .get::<_, i64>(0))
            .is_err());

        // The right passphrase can.
        let key = vault.unlock(PASSPHRASE).expect("unlock");
        let text: String = vault
            .connect(&key)
            .expect("connect")
            .query_row("SELECT text FROM notes", [], |r| r.get(0))
            .expect("read");
        assert_eq!(text, "réunion secrète avec Ahmed");
    }

    #[cfg(unix)]
    #[test]
    fn the_header_is_private_and_holds_no_key() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, vault) = vault();
        let key = vault.create(PASSPHRASE).expect("create");
        let path = dir.path().join(VAULT_FILE);
        let mode = |p: &Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir.path().join(DB_FILE)), 0o600);
        assert_eq!(mode(dir.path()), 0o700);
        let text = std::fs::read_to_string(&path).expect("read header");
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!text.contains(&hex));
        assert!(!text.contains(&B64.encode(*key)));
        assert!(!text.contains(PASSPHRASE));
    }
}
