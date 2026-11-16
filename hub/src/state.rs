//! Shared state of the running hub: the vault and, once unlocked, its key.

use crate::vault::{Vault, VaultError, VaultKey};
use crate::{db, devices};
use anyhow::Result;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lock state as reported by `GET /api/status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockState {
    /// No vault yet: the hub is waiting for its first passphrase.
    Uninitialized,
    /// The vault exists but the key is not in memory.
    Locked,
    /// The key is in memory and data can be read and written.
    Unlocked,
}

/// After a wrong passphrase, further attempts are refused for a while. The
/// wait doubles with each failure, up to this cap.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Default)]
struct Inner {
    key: Option<VaultKey>,
    failed_attempts: u32,
    retry_after: Option<Instant>,
}

/// Handle shared by all request handlers.
#[derive(Clone)]
pub struct AppState {
    vault: Vault,
    inner: Arc<Mutex<Inner>>,
}

/// Why a passphrase attempt was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum UnlockError {
    #[error("too many wrong attempts; try again in {0} s")]
    Throttled(u64),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

impl AppState {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn lock_state(&self) -> LockState {
        if self.inner().key.is_some() {
            LockState::Unlocked
        } else if self.vault.is_initialized() {
            LockState::Locked
        } else {
            LockState::Uninitialized
        }
    }

    fn check_throttle(&self) -> Result<(), UnlockError> {
        if let Some(until) = self.inner().retry_after {
            let now = Instant::now();
            if until > now {
                return Err(UnlockError::Throttled((until - now).as_secs().max(1)));
            }
        }
        Ok(())
    }

    fn note_result<T>(&self, result: &Result<T, VaultError>) {
        let mut inner = self.inner();
        match result {
            Err(VaultError::WrongPassphrase) => {
                inner.failed_attempts += 1;
                let wait =
                    Duration::from_secs(1u64 << inner.failed_attempts.min(9)).min(MAX_BACKOFF);
                inner.retry_after = Some(Instant::now() + wait);
            }
            Ok(_) => {
                inner.failed_attempts = 0;
                inner.retry_after = None;
            }
            Err(_) => {}
        }
    }

    fn install(&self, key: VaultKey) -> Result<(), VaultError> {
        let mut conn = self.vault.connect(&key)?;
        db::migrate(&mut conn)?;
        self.inner().key = Some(key);
        Ok(())
    }

    /// Creates the vault with its first passphrase and unlocks it.
    pub fn initialize(&self, passphrase: &str) -> Result<(), UnlockError> {
        let key = self.vault.create(passphrase)?;
        Ok(self.install(key)?)
    }

    /// Unlocks the vault. Does nothing if it is already unlocked and the
    /// passphrase is right.
    pub fn unlock(&self, passphrase: &str) -> Result<(), UnlockError> {
        self.check_throttle()?;
        let result = self.vault.unlock(passphrase);
        self.note_result(&result);
        Ok(self.install(result?)?)
    }

    /// Checks a passphrase without changing the lock state. Used to
    /// authorize administration, such as registering a device.
    pub fn verify_passphrase(&self, passphrase: &str) -> Result<(), UnlockError> {
        self.check_throttle()?;
        let result = self.vault.unlock(passphrase);
        self.note_result(&result);
        result?;
        Ok(())
    }

    /// Forgets the key. Data is unreadable until the next unlock.
    pub fn lock(&self) {
        self.inner().key = None;
    }

    /// A database connection, or `None` while the hub is locked.
    pub fn connection(&self) -> Result<Option<Connection>> {
        let inner = self.inner();
        match &inner.key {
            Some(key) => Ok(Some(self.vault.connect(key)?)),
            None => Ok(None),
        }
    }

    /// Writes a backup snapshot if the hub is unlocked. Returns `None` while
    /// locked: nothing new can have been written then.
    pub fn snapshot(&self, stamp: &str) -> Result<Option<std::path::PathBuf>> {
        match self.connection()? {
            Some(conn) => Ok(Some(crate::snapshot::take(
                &conn,
                &self.vault.snapshot_dir(),
                stamp,
            )?)),
            None => Ok(None),
        }
    }

    /// The device a bearer token belongs to. `Ok(None)` when the token is
    /// unknown or revoked; `Err(Locked)` semantics are the caller's: check
    /// [`AppState::lock_state`] first.
    pub fn authenticate(&self, token: &str) -> Result<Option<devices::Device>> {
        match self.connection()? {
            Some(conn) => devices::authenticate(&conn, token),
            None => Ok(None),
        }
    }
}
