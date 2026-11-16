//! Sends this device's dictations, rules and corrections to the hub, and
//! brings rule changes made on other devices back.
//!
//! Dictations are copied into a queue (`sync_outbox`) the moment they are
//! saved, because Handy prunes its own history. A queued dictation leaves the
//! queue only once the hub has accepted it. Nothing is queued while no hub is
//! connected; connecting queues the history that exists at that moment.
//!
//! The hub answers `423 Locked` after it restarts, until its passphrase is
//! entered. That is not an error here: the queue simply waits.

use super::store::{now_ms, LearningStore};
use anyhow::{Context, Result};
use hikaya_core::sync::{
    DictationRecord, MeetingLineRecord, MeetingRecord, PullResponse, PushRequest, PushResponse,
    RuleRecord, SubstitutionRecord,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::future::Future;
use std::time::Duration;

/// Dictations sent per push. Rules and corrections ride along.
const DICTATIONS_PER_PUSH: usize = 200;
/// Meetings sent per push: each carries its whole transcript.
const MEETINGS_PER_PUSH: usize = 3;
/// Meetings in these states are finished and can be sent.
const FINISHED: &str = "('done', 'interrupted')";
/// Safety stop for one sync run; the rest goes in the next run.
const MAX_PUSHES_PER_RUN: usize = 20;
/// How long a request may take before the hub counts as unreachable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// What went wrong talking to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubError {
    /// No answer: wrong address, no network, or the hub is down.
    Offline(String),
    /// The hub restarted and waits for its passphrase.
    Locked,
    /// The device token is unknown or was revoked.
    Unauthorized,
    /// The hub refused the request, with its reason.
    Rejected(String),
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline(why) => write!(f, "the hub can't be reached: {why}"),
            Self::Locked => write!(f, "the hub is locked"),
            Self::Unauthorized => write!(f, "this device is not allowed on the hub"),
            Self::Rejected(why) => write!(f, "the hub refused: {why}"),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for HubError {}

/// How this device reaches the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubConfig {
    pub url: String,
    pub token: String,
    pub device_name: String,
    /// Prefix of this device's dictation ids, unique per installation.
    pub device_uid: String,
}

/// The hub operations sync needs. A trait so tests can stand in for the hub.
pub trait HubApi {
    fn push(&self, request: &PushRequest) -> impl Future<Output = Result<PushResponse, HubError>>;
    fn pull(&self, since: i64) -> impl Future<Output = Result<PullResponse, HubError>>;
}

/// What one sync run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub sent_dictations: usize,
    pub sent_rules: usize,
    pub received_rules: usize,
}

// ---------------------------------------------------------------- storage --

fn setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM sync_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

impl LearningStore {
    /// The hub this device is connected to, if any.
    pub fn hub_config(&self) -> Result<Option<HubConfig>> {
        let conn = self.conn()?;
        let (Some(url), Some(token)) = (setting(&conn, "hub_url")?, setting(&conn, "hub_token")?)
        else {
            return Ok(None);
        };
        Ok(Some(HubConfig {
            url,
            token,
            device_name: setting(&conn, "device_name")?.unwrap_or_default(),
            device_uid: self.device_uid_with(&conn)?,
        }))
    }

    fn device_uid_with(&self, conn: &Connection) -> Result<String> {
        if let Some(uid) = setting(conn, "device_uid")? {
            return Ok(uid);
        }
        let uid: String =
            conn.query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        set_setting(conn, "device_uid", &uid)?;
        Ok(uid)
    }

    /// Saves the connection to a hub.
    pub fn save_hub_config(&self, url: &str, token: &str, device_name: &str) -> Result<HubConfig> {
        let conn = self.conn()?;
        set_setting(&conn, "hub_url", url)?;
        set_setting(&conn, "hub_token", token)?;
        set_setting(&conn, "device_name", device_name)?;
        // A different hub has its own feed: start pulling from the beginning.
        conn.execute("DELETE FROM sync_settings WHERE key = 'pull_cursor'", [])?;
        drop(conn);
        self.hub_config()?
            .context("the hub settings were not saved")
    }

    /// Forgets the hub and empties the queue. Rules stay on this device.
    pub fn clear_hub_config(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "DELETE FROM sync_settings WHERE key IN ('hub_url', 'hub_token', 'pull_cursor', 'last_sync_at');
             DELETE FROM sync_outbox;",
        )?;
        Ok(())
    }

    /// Queues a dictation (or its corrected text) for the hub. Does nothing
    /// while no hub is connected.
    pub fn queue_dictation(
        &self,
        history_id: i64,
        created_at_ms: i64,
        text: &str,
        language: Option<&str>,
    ) -> Result<bool> {
        let Some(config) = self.hub_config()? else {
            return Ok(false);
        };
        if text.trim().is_empty() {
            return Ok(false);
        }
        let uid = format!("{}-{history_id}", config.device_uid);
        // updated_at can never be before created_at, even if the clock moved.
        let updated_at = now_ms().max(created_at_ms);
        self.conn()?.execute(
            "INSERT INTO sync_outbox (uid, history_id, created_at, updated_at, text, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (uid) DO UPDATE SET text = ?5, updated_at = ?4, language = ?6",
            params![uid, history_id, created_at_ms, updated_at, text, language],
        )?;
        Ok(true)
    }

    /// Dictations, rules and corrections waiting to be sent.
    pub fn pending_count(&self) -> Result<u32> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT (SELECT count(*) FROM sync_outbox)
                  + (SELECT count(*) FROM rules WHERE synced = 0)
                  + (SELECT count(*) FROM substitutions WHERE synced = 0)
                  + (SELECT count(*) FROM meetings WHERE synced = 0 AND status IN ('done', 'interrupted'))",
            [],
            |row| row.get(0),
        )?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Unix ms of the last successful sync.
    pub fn last_sync_at(&self) -> Result<Option<i64>> {
        Ok(setting(&self.conn()?, "last_sync_at")?.and_then(|v| v.parse().ok()))
    }

    fn next_batch(&self) -> Result<PushRequest> {
        let conn = self.conn()?;
        let dictations = conn
            .prepare(
                "SELECT uid, created_at, updated_at, text, language FROM sync_outbox
                 ORDER BY created_at LIMIT ?1",
            )?
            .query_map(params![DICTATIONS_PER_PUSH as i64], |row| {
                Ok(DictationRecord {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    updated_at: row.get(2)?,
                    text: row.get(3)?,
                    language: row.get(4)?,
                    duration_ms: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let rules = conn
            .prepare(
                "SELECT uid, pattern, replacement, enabled, source, updated_at, deleted
                 FROM rules WHERE synced = 0 LIMIT 100",
            )?
            .query_map([], |row| {
                Ok(RuleRecord {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                    replacement: row.get(2)?,
                    enabled: row.get(3)?,
                    source: row.get(4)?,
                    updated_at: row.get(5)?,
                    deleted: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let substitutions = conn
            .prepare(
                "SELECT uid, from_text, to_text, created_at FROM substitutions
                 WHERE synced = 0 LIMIT 100",
            )?
            .query_map([], |row| {
                Ok(SubstitutionRecord {
                    id: row.get(0)?,
                    from: row.get(1)?,
                    to: row.get(2)?,
                    // Stored in seconds; the wire uses milliseconds.
                    created_at: row.get::<_, i64>(3)? * 1000,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let finished = conn
            .prepare(&format!(
                "SELECT id, uid, title, started_at, duration_ms, updated_at, summary FROM meetings
                 WHERE synced = 0 AND status IN {FINISHED} ORDER BY started_at LIMIT ?1"
            ))?
            .query_map(params![MEETINGS_PER_PUSH as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    MeetingRecord {
                        id: row.get(1)?,
                        title: row.get(2)?,
                        started_at: row.get(3)?,
                        duration_ms: row.get(4)?,
                        updated_at: row.get(5)?,
                        summary: row.get(6)?,
                        lines: Vec::new(),
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(conn);
        let mut meetings = Vec::with_capacity(finished.len());
        for (id, mut meeting) in finished {
            meeting.lines = self
                .meeting_lines(id)?
                .into_iter()
                .map(|l| MeetingLineRecord {
                    start_ms: l.start_ms,
                    end_ms: l.end_ms,
                    speaker: match l.speaker {
                        hikaya_core::meeting::Speaker::You => "you".into(),
                        hikaya_core::meeting::Speaker::Others => "others".into(),
                    },
                    text: l.text,
                })
                .collect();
            meetings.push(meeting);
        }
        Ok(PushRequest {
            dictations,
            rules,
            substitutions,
            meetings,
        })
    }

    /// Marks what the hub accepted. A record changed while the push was in
    /// flight keeps its newer state and is sent again next time.
    fn mark_sent(&self, batch: &PushRequest) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for d in &batch.dictations {
            tx.execute(
                "DELETE FROM sync_outbox WHERE uid = ?1 AND updated_at = ?2",
                params![d.id, d.updated_at],
            )?;
        }
        for r in &batch.rules {
            tx.execute(
                "UPDATE rules SET synced = 1 WHERE uid = ?1 AND updated_at = ?2",
                params![r.id, r.updated_at],
            )?;
        }
        for m in &batch.meetings {
            tx.execute(
                "UPDATE meetings SET synced = 1 WHERE uid = ?1 AND updated_at = ?2",
                params![m.id, m.updated_at],
            )?;
        }
        // Tombstones the hub has seen are no longer needed here.
        tx.execute("DELETE FROM rules WHERE deleted = 1 AND synced = 1", [])?;
        for s in &batch.substitutions {
            tx.execute(
                "UPDATE substitutions SET synced = 1 WHERE uid = ?1",
                params![s.id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Applies rule changes from the hub. The newest change wins; a local
    /// change that has not been sent yet and is newer is kept.
    fn merge_rules(&self, rules: &[RuleRecord]) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut applied = 0;
        for rule in rules {
            let local: Option<i64> = tx
                .query_row(
                    "SELECT updated_at FROM rules WHERE uid = ?1",
                    params![rule.id],
                    |row| row.get(0),
                )
                .optional()?;
            match local {
                Some(updated_at) if updated_at >= rule.updated_at => continue,
                Some(_) => {
                    tx.execute(
                        "UPDATE rules SET pattern = ?2, replacement = ?3, enabled = ?4, source = ?5,
                           updated_at = ?6, deleted = ?7, synced = 1 WHERE uid = ?1",
                        params![
                            rule.id,
                            rule.pattern,
                            rule.replacement,
                            rule.enabled,
                            rule.source,
                            rule.updated_at,
                            rule.deleted
                        ],
                    )?;
                }
                None if rule.deleted => continue,
                None => {
                    tx.execute(
                        "INSERT INTO rules (uid, pattern, replacement, enabled, source, created_at, updated_at, synced)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                        params![
                            rule.id,
                            rule.pattern,
                            rule.replacement,
                            rule.enabled,
                            rule.source,
                            rule.updated_at / 1000,
                            rule.updated_at
                        ],
                    )?;
                }
            }
            applied += 1;
        }
        tx.execute("DELETE FROM rules WHERE deleted = 1 AND synced = 1", [])?;
        tx.commit()?;
        drop(conn);
        self.refresh_active()?;
        Ok(applied)
    }

    fn pull_cursor(&self) -> Result<i64> {
        Ok(setting(&self.conn()?, "pull_cursor")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }
}

/// One full exchange with the hub: push everything queued, then pull.
pub async fn sync_once(store: &LearningStore, hub: &impl HubApi) -> Result<SyncReport, HubError> {
    let other = |e: anyhow::Error| HubError::Other(format!("{e:#}"));
    let mut report = SyncReport::default();
    for _ in 0..MAX_PUSHES_PER_RUN {
        let batch = store.next_batch().map_err(other)?;
        if batch.is_empty() {
            break;
        }
        hub.push(&batch).await?;
        store.mark_sent(&batch).map_err(other)?;
        report.sent_dictations += batch.dictations.len();
        report.sent_rules += batch.rules.len();
        if batch.meetings.len() < MEETINGS_PER_PUSH
            && batch.dictations.len() < DICTATIONS_PER_PUSH
            && batch.rules.len() < 100
            && batch.substitutions.len() < 100
        {
            break;
        }
    }

    let pulled = hub.pull(store.pull_cursor().map_err(other)?).await?;
    report.received_rules = store.merge_rules(&pulled.rules).map_err(other)?;
    let conn = store.conn().map_err(other)?;
    set_setting(&conn, "pull_cursor", &pulled.cursor.to_string()).map_err(other)?;
    set_setting(&conn, "last_sync_at", &now_ms().to_string()).map_err(other)?;
    Ok(report)
}

// ------------------------------------------------------------ HTTP client --

/// The real hub, over HTTP.
pub struct HttpHub {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

/// The hub's lock state, as `GET /api/status` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum HubLockState {
    Uninitialized,
    Locked,
    Unlocked,
}

#[derive(Deserialize)]
struct StatusBody {
    state: HubLockState,
}

#[derive(Deserialize)]
struct RegisterBody {
    token: String,
}

/// Checks and normalizes a hub address typed by the user.
pub fn normalize_url(url: &str) -> Result<String, HubError> {
    let url = url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| HubError::Rejected("the address is not a valid URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(HubError::Rejected(
            "the address must start with http:// or https://".into(),
        ));
    }
    Ok(url.to_string())
}

impl HttpHub {
    pub fn new(base: &str, token: Option<&str>) -> Result<Self, HubError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Hikaya/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HubError::Other(e.to_string()))?;
        Ok(Self {
            base: normalize_url(base)?,
            token: token.map(str::to_string),
            http,
        })
    }

    pub fn for_config(config: &HubConfig) -> Result<Self, HubError> {
        Self::new(&config.url, Some(&config.token))
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, HubError> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|e| HubError::Offline(e.without_url().to_string()))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|e| HubError::Other(format!("unexpected answer from the hub: {e}")));
        }
        let message = response
            .json::<ErrorBody>()
            .await
            .ok()
            .and_then(|b| b.message)
            .unwrap_or_else(|| status.to_string());
        Err(match status.as_u16() {
            423 => HubError::Locked,
            401 => HubError::Unauthorized,
            400..=499 => HubError::Rejected(message),
            _ => HubError::Other(message),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn status(&self) -> Result<HubLockState, HubError> {
        let body: StatusBody = self.send(self.http.get(self.url("/api/status"))).await?;
        Ok(body.state)
    }

    /// Creates the hub's vault. Only works once, on a new hub.
    pub async fn initialize(&self, passphrase: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url("/api/init"))
                    .json(&serde_json::json!({ "passphrase": passphrase })),
            )
            .await?;
        Ok(())
    }

    pub async fn unlock(&self, passphrase: &str) -> Result<(), HubError> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url("/api/unlock"))
                    .json(&serde_json::json!({ "passphrase": passphrase })),
            )
            .await?;
        Ok(())
    }

    pub async fn register(&self, passphrase: &str, name: &str) -> Result<String, HubError> {
        let body: RegisterBody = self
            .send(
                self.http
                    .post(self.url("/api/devices"))
                    .json(&serde_json::json!({ "passphrase": passphrase, "name": name })),
            )
            .await?;
        Ok(body.token)
    }
}

impl HubApi for HttpHub {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, HubError> {
        self.send(self.http.post(self.url("/api/sync/push")).json(request))
            .await
    }

    async fn pull(&self, since: i64) -> Result<PullResponse, HubError> {
        self.send(
            self.http
                .get(self.url("/api/sync/pull"))
                .query(&[("since", since)]),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An in-memory hub with the same rules as the real one.
    #[derive(Default)]
    struct FakeHub {
        dictations: Mutex<Vec<DictationRecord>>,
        meetings: Mutex<Vec<MeetingRecord>>,
        rules: Mutex<Vec<(i64, RuleRecord)>>,
        error: Mutex<Option<HubError>>,
        pushes: Mutex<usize>,
    }

    impl FakeHub {
        fn fail_with(&self, error: Option<HubError>) {
            *self.error.lock().expect("lock") = error;
        }
        fn texts(&self) -> Vec<String> {
            self.dictations
                .lock()
                .expect("lock")
                .iter()
                .map(|d| d.text.clone())
                .collect()
        }
        fn add_remote_rule(&self, rule: RuleRecord) {
            let mut rules = self.rules.lock().expect("lock");
            let seq = rules.len() as i64 + 1;
            rules.retain(|(_, r)| r.id != rule.id);
            rules.push((seq, rule));
        }
    }

    impl HubApi for FakeHub {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, HubError> {
            if let Some(error) = self.error.lock().expect("lock").clone() {
                return Err(error);
            }
            *self.pushes.lock().expect("lock") += 1;
            let mut dictations = self.dictations.lock().expect("lock");
            for d in &request.dictations {
                match dictations.iter_mut().find(|x| x.id == d.id) {
                    Some(x) if x.updated_at < d.updated_at => *x = d.clone(),
                    Some(_) => {}
                    None => dictations.push(d.clone()),
                }
            }
            drop(dictations);
            let mut meetings = self.meetings.lock().expect("lock");
            for m in &request.meetings {
                match meetings.iter_mut().find(|x| x.id == m.id) {
                    Some(x) if x.updated_at < m.updated_at => *x = m.clone(),
                    Some(_) => {}
                    None => meetings.push(m.clone()),
                }
            }
            drop(meetings);
            for r in &request.rules {
                let newer = self
                    .rules
                    .lock()
                    .expect("lock")
                    .iter()
                    .find(|(_, x)| x.id == r.id)
                    .is_none_or(|(_, x)| x.updated_at < r.updated_at);
                if newer {
                    self.add_remote_rule(r.clone());
                }
            }
            Ok(PushResponse::default())
        }

        async fn pull(&self, since: i64) -> Result<PullResponse, HubError> {
            if let Some(error) = self.error.lock().expect("lock").clone() {
                return Err(error);
            }
            let rules = self.rules.lock().expect("lock");
            let changed: Vec<&(i64, RuleRecord)> =
                rules.iter().filter(|(s, _)| *s > since).collect();
            Ok(PullResponse {
                cursor: changed.iter().map(|(s, _)| *s).max().unwrap_or(since),
                rules: changed.into_iter().map(|(_, r)| r.clone()).collect(),
            })
        }
    }

    fn store() -> (tempfile::TempDir, LearningStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LearningStore::open(&dir.path().join("hikaya.db")).expect("open");
        (dir, store)
    }

    fn connected() -> (tempfile::TempDir, LearningStore) {
        let (dir, store) = store();
        store
            .save_hub_config("http://hub.test", "hky_token", "laptop")
            .expect("connect");
        (dir, store)
    }

    fn remote_rule(id: &str, at: i64, pattern: &str, replacement: &str) -> RuleRecord {
        RuleRecord {
            id: id.into(),
            pattern: pattern.into(),
            replacement: replacement.into(),
            enabled: true,
            source: "manual".into(),
            updated_at: at,
            deleted: false,
        }
    }

    #[tokio::test]
    async fn nothing_is_queued_until_a_hub_is_connected() {
        let (_dir, store) = store();
        assert!(!store
            .queue_dictation(1, 1000, "bonjour", Some("fr"))
            .expect("queue"));
        assert_eq!(store.pending_count().expect("count"), 0);
        assert_eq!(store.hub_config().expect("config"), None);
    }

    #[tokio::test]
    async fn queued_dictations_reach_the_hub_and_leave_the_queue() {
        let (_dir, store) = connected();
        assert!(store
            .queue_dictation(1, 1000, "bonjour", Some("fr"))
            .expect("queue"));
        store
            .queue_dictation(2, 2000, "hello", None)
            .expect("queue");
        assert_eq!(store.pending_count().expect("count"), 2);

        let hub = FakeHub::default();
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.sent_dictations, 2);
        assert_eq!(hub.texts(), ["bonjour", "hello"]);
        assert_eq!(store.pending_count().expect("count"), 0);
        assert!(store.last_sync_at().expect("last").is_some());
    }

    #[tokio::test]
    async fn a_locked_or_unreachable_hub_keeps_the_queue() {
        let (_dir, store) = connected();
        store
            .queue_dictation(1, 1000, "bonjour", None)
            .expect("queue");
        let hub = FakeHub::default();
        for error in [HubError::Locked, HubError::Offline("down".into())] {
            hub.fail_with(Some(error.clone()));
            assert_eq!(sync_once(&store, &hub).await, Err(error));
            assert_eq!(store.pending_count().expect("count"), 1);
        }
        hub.fail_with(None);
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts(), ["bonjour"]);
    }

    #[tokio::test]
    async fn a_correction_is_sent_as_a_newer_version_of_the_same_dictation() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store
            .queue_dictation(7, 1000, "merci a med", None)
            .expect("queue");
        sync_once(&store, &hub).await.expect("sync");
        std::thread::sleep(Duration::from_millis(2));
        store
            .queue_dictation(7, 1000, "merci Ahmed", None)
            .expect("correct");
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts(), ["merci Ahmed"]);
    }

    #[tokio::test]
    async fn rules_go_up_and_come_down_with_the_newest_change_winning() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let local = store.add_rule("get hub", "GitHub", "manual").expect("add");
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.sent_rules, 1);

        // Another device adds a rule and edits ours later.
        hub.add_remote_rule(remote_rule("phone-1", now_ms(), "clod", "Claude"));
        let ours = hub.rules.lock().expect("lock")[0].1.clone();
        hub.add_remote_rule(RuleRecord {
            replacement: "GitHub.com".into(),
            updated_at: local.updated_at + 10_000,
            ..ours
        });
        let report = sync_once(&store, &hub).await.expect("sync");
        assert_eq!(report.received_rules, 2);
        let mut rules: Vec<(String, String)> = store
            .list_rules()
            .expect("list")
            .into_iter()
            .map(|r| (r.pattern, r.replacement))
            .collect();
        rules.sort();
        assert_eq!(
            rules,
            [
                ("clod".to_string(), "Claude".to_string()),
                ("get hub".to_string(), "GitHub.com".to_string())
            ]
        );
        // Received rules are applied to the next dictation.
        assert_eq!(store.active_rules().len(), 2);
        assert_eq!(store.pending_count().expect("count"), 0);
    }

    #[tokio::test]
    async fn an_older_remote_change_does_not_overwrite_a_newer_local_one() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let local = store.add_rule("clod", "Claude", "manual").expect("add");
        let uid: String = store
            .conn()
            .expect("conn")
            .query_row("SELECT uid FROM rules", [], |r| r.get(0))
            .expect("uid");
        hub.add_remote_rule(remote_rule(&uid, local.updated_at - 5_000, "clod", "cloud"));
        // Pull happens after push in the same run; the hub keeps the newer one.
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(store.list_rules().expect("list")[0].replacement, "Claude");
    }

    #[tokio::test]
    async fn a_deletion_reaches_the_hub_and_the_tombstone_is_then_dropped() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let rule = store.add_rule("clod", "Claude", "manual").expect("add");
        sync_once(&store, &hub).await.expect("sync");
        std::thread::sleep(Duration::from_millis(2));
        store.delete_rule(rule.id).expect("delete");
        assert!(store.list_rules().expect("list").is_empty());
        sync_once(&store, &hub).await.expect("sync");
        assert!(hub.rules.lock().expect("lock")[0].1.deleted);
        let left: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM rules", [], |r| r.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_remote_deletion_removes_the_local_rule() {
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        store.add_rule("clod", "Claude", "manual").expect("add");
        sync_once(&store, &hub).await.expect("sync");
        let mut r = hub.rules.lock().expect("lock")[0].1.clone();
        r.deleted = true;
        r.updated_at += 10_000;
        hub.add_remote_rule(r);
        sync_once(&store, &hub).await.expect("sync");
        assert!(store.list_rules().expect("list").is_empty());
        assert!(store.active_rules().is_empty());
    }

    #[tokio::test]
    async fn a_large_backlog_is_sent_in_several_pushes() {
        let (_dir, store) = connected();
        for id in 0..450 {
            store
                .queue_dictation(id, 1000 + id, &format!("dictée {id}"), None)
                .expect("queue");
        }
        let hub = FakeHub::default();
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(hub.texts().len(), 450);
        assert_eq!(*hub.pushes.lock().expect("lock"), 3);
        assert_eq!(store.pending_count().expect("count"), 0);
    }

    #[tokio::test]
    async fn disconnecting_empties_the_queue_but_keeps_rules() {
        let (_dir, store) = connected();
        store.add_rule("clod", "Claude", "manual").expect("add");
        store
            .queue_dictation(1, 1000, "bonjour", None)
            .expect("queue");
        store.clear_hub_config().expect("disconnect");
        assert_eq!(store.hub_config().expect("config"), None);
        assert_eq!(store.list_rules().expect("list").len(), 1);
        let queued: i64 = store
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM sync_outbox", [], |r| r.get(0))
            .expect("count");
        assert_eq!(queued, 0);
    }

    #[tokio::test]
    async fn a_finished_meeting_is_sent_whole_and_again_when_it_changes() {
        use crate::hikaya::meeting::store::MeetingStatus;
        use hikaya_core::meeting::{Speaker, TranscriptLine};
        let (_dir, store) = connected();
        let hub = FakeHub::default();
        let id = store
            .create_meeting("Point produit", 1_000, true)
            .expect("create");
        for (start, speaker, text) in [
            (0, Speaker::Others, "On décale ?"),
            (30_000, Speaker::You, "Oui, jeudi."),
        ] {
            store
                .add_meeting_line(
                    id,
                    &TranscriptLine {
                        start_ms: start,
                        end_ms: start + 30_000,
                        speaker,
                        text: text.into(),
                    },
                )
                .expect("line");
        }
        // Still recording: not sent.
        sync_once(&store, &hub).await.expect("sync");
        assert!(hub.meetings.lock().expect("lock").is_empty());

        store
            .set_meeting_status(id, MeetingStatus::Done, 60_000)
            .expect("done");
        assert_eq!(store.pending_count().expect("pending"), 1);
        sync_once(&store, &hub).await.expect("sync");
        {
            let sent = hub.meetings.lock().expect("lock");
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].title, "Point produit");
            assert_eq!(sent[0].lines.len(), 2);
            assert_eq!(sent[0].lines[0].speaker, "others");
        }
        assert_eq!(store.pending_count().expect("pending"), 0);

        std::thread::sleep(Duration::from_millis(2));
        store
            .set_meeting_summary(id, "## TL;DR\nDémo jeudi.", "test")
            .expect("summary");
        assert_eq!(store.pending_count().expect("pending"), 1);
        sync_once(&store, &hub).await.expect("sync");
        assert_eq!(
            hub.meetings.lock().expect("lock")[0].summary.as_deref(),
            Some("## TL;DR\nDémo jeudi.")
        );
    }

    #[test]
    fn hub_addresses_are_checked() {
        assert_eq!(
            normalize_url(" https://hub.example.ts.net/ ").expect("ok"),
            "https://hub.example.ts.net"
        );
        assert_eq!(
            normalize_url("http://100.64.0.1:8787").expect("ok"),
            "http://100.64.0.1:8787"
        );
        for bad in [
            "",
            "hub.local",
            "ftp://hub",
            "javascript:alert(1)",
            "http://",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_device_uid_is_stable() {
        let (_dir, store) = connected();
        let first = store
            .hub_config()
            .expect("config")
            .expect("some")
            .device_uid;
        let again = store
            .hub_config()
            .expect("config")
            .expect("some")
            .device_uid;
        assert_eq!(first, again);
        assert_eq!(first.len(), 16);
    }
}
