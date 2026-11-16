//! The hub's HTTP API.
//!
//! | Route                       | Who may call it       | What it does                         |
//! | --------------------------- | --------------------- | ------------------------------------ |
//! | `GET  /api/status`          | anyone on the network | lock state and version               |
//! | `POST /api/init`            | anyone, once          | creates the vault with a passphrase  |
//! | `POST /api/unlock`          | passphrase            | loads the key into memory            |
//! | `POST /api/lock`            | a device              | forgets the key                      |
//! | `POST /api/devices`         | passphrase            | registers a device, returns a token  |
//! | `GET  /api/devices`         | a device              | lists devices                        |
//! | `DELETE /api/devices/{id}`  | passphrase            | revokes a device                     |
//! | `GET  /api/me`              | a device              | the calling device                   |
//! | `POST /api/sync/push`       | a device              | sends dictations, rules, corrections |
//! | `GET  /api/sync/pull`       | a device              | rule changes since a cursor          |
//! | `GET  /api/dictations`      | a device              | lists or searches the archive        |
//! | `GET  /api/meetings`        | a device              | lists or searches meetings           |
//! | `GET  /api/meetings/{id}`   | a device              | one meeting with its transcript      |
//!
//! While the hub is locked, anything that needs data answers `423 Locked`,
//! which tells a client to keep its changes queued and try again later.

use crate::devices::{self, Device};
use crate::state::{AppState, LockState, UnlockError};
use crate::sync::{self, PushError};
use crate::vault::VaultError;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use hikaya_core::sync::{PullResponse, PushRequest, PushResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// An API error: a status code and a message safe to show the user.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn locked() -> Self {
        Self::new(
            StatusCode::LOCKED,
            "locked",
            "the hub is locked; unlock it with the passphrase",
        )
    }

    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid device token is required",
        )
    }

    fn internal(error: anyhow::Error) -> Self {
        // The detail goes to the log, never to the client.
        tracing::error!("internal error: {error:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the hub hit an internal error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.code, "message": self.message })),
        )
            .into_response()
    }
}

impl From<UnlockError> for ApiError {
    fn from(error: UnlockError) -> Self {
        match error {
            UnlockError::Throttled(seconds) => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "throttled",
                format!("too many wrong attempts; try again in {seconds} s"),
            ),
            UnlockError::Vault(VaultError::WrongPassphrase) => Self::new(
                StatusCode::FORBIDDEN,
                "wrong_passphrase",
                "the passphrase is wrong",
            ),
            UnlockError::Vault(VaultError::NotInitialized) => Self::new(
                StatusCode::CONFLICT,
                "not_initialized",
                "the hub has no vault yet; create one first",
            ),
            UnlockError::Vault(VaultError::AlreadyInitialized) => Self::new(
                StatusCode::CONFLICT,
                "already_initialized",
                "the vault already exists",
            ),
            UnlockError::Vault(error @ VaultError::WeakPassphrase) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "weak_passphrase",
                error.to_string(),
            ),
            UnlockError::Vault(VaultError::Other(error)) => Self::internal(error),
        }
    }
}

/// Extractor: the device that sent the request, proven by its bearer token.
pub struct AuthedDevice(pub Device);

impl FromRequestParts<AppState> for AuthedDevice {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if state.lock_state() != LockState::Unlocked {
            return Err(ApiError::locked());
        }
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        match state.authenticate(token).map_err(ApiError::internal)? {
            Some(device) => Ok(Self(device)),
            None => Err(ApiError::unauthorized()),
        }
    }
}

#[derive(Deserialize)]
struct PassphraseBody {
    passphrase: String,
}

#[derive(Deserialize)]
struct NewDeviceBody {
    passphrase: String,
    name: String,
}

#[derive(Serialize)]
struct StatusBody {
    state: LockState,
    version: &'static str,
}

fn status_body(state: &AppState) -> Json<StatusBody> {
    Json(StatusBody {
        state: state.lock_state(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn status(State(state): State<AppState>) -> Json<StatusBody> {
    status_body(&state)
}

/// Passphrase work (Argon2) is slow on purpose; keep it off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("worker failed: {e}")))?
}

async fn init(
    State(state): State<AppState>,
    Json(body): Json<PassphraseBody>,
) -> Result<(StatusCode, Json<StatusBody>), ApiError> {
    let worker = state.clone();
    blocking(move || Ok(worker.initialize(&body.passphrase)?)).await?;
    tracing::info!("vault created and unlocked");
    Ok((StatusCode::CREATED, status_body(&state)))
}

async fn unlock(
    State(state): State<AppState>,
    Json(body): Json<PassphraseBody>,
) -> Result<Json<StatusBody>, ApiError> {
    let worker = state.clone();
    blocking(move || Ok(worker.unlock(&body.passphrase)?)).await?;
    tracing::info!("hub unlocked");
    Ok(status_body(&state))
}

async fn lock(State(state): State<AppState>, _device: AuthedDevice) -> Json<StatusBody> {
    state.lock();
    tracing::info!("hub locked on request");
    status_body(&state)
}

async fn register_device(
    State(state): State<AppState>,
    Json(body): Json<NewDeviceBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if state.lock_state() != LockState::Unlocked {
        return Err(ApiError::locked());
    }
    let worker = state.clone();
    let (device, token) = blocking(move || {
        worker.verify_passphrase(&body.passphrase)?;
        let conn = worker
            .connection()
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::locked)?;
        devices::register(&conn, &body.name).map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_device",
                e.to_string(),
            )
        })
    })
    .await?;
    tracing::info!(device = %device.name, "device registered");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "device": device, "token": token })),
    ))
}

async fn list_devices(
    State(state): State<AppState>,
    _device: AuthedDevice,
) -> Result<Json<Vec<Device>>, ApiError> {
    let conn = state
        .connection()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::locked)?;
    Ok(Json(devices::list(&conn).map_err(ApiError::internal)?))
}

async fn revoke_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<PassphraseBody>,
) -> Result<StatusCode, ApiError> {
    if state.lock_state() != LockState::Unlocked {
        return Err(ApiError::locked());
    }
    let worker = state.clone();
    let found = blocking(move || {
        worker.verify_passphrase(&body.passphrase)?;
        let conn = worker
            .connection()
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::locked)?;
        devices::revoke(&conn, id).map_err(ApiError::internal)
    })
    .await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such device",
        ))
    }
}

async fn me(AuthedDevice(device): AuthedDevice) -> Json<Device> {
    Json(device)
}

impl From<PushError> for ApiError {
    fn from(error: PushError) -> Self {
        match error {
            PushError::TooLarge => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                error.to_string(),
            ),
            PushError::Invalid(..) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_record",
                error.to_string(),
            ),
            PushError::NotYours(_) => {
                Self::new(StatusCode::CONFLICT, "not_yours", error.to_string())
            }
            PushError::Other(error) => Self::internal(error),
        }
    }
}

fn data_connection(state: &AppState) -> Result<rusqlite::Connection, ApiError> {
    state
        .connection()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::locked)
}

async fn sync_push(
    State(state): State<AppState>,
    AuthedDevice(device): AuthedDevice,
    Json(request): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let response = blocking(move || {
        let mut conn = data_connection(&state)?;
        Ok(sync::push(&mut conn, device.id, &request)?)
    })
    .await?;
    Ok(Json(response))
}

#[derive(Deserialize)]
struct PullQuery {
    #[serde(default)]
    since: i64,
}

async fn sync_pull(
    State(state): State<AppState>,
    _device: AuthedDevice,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResponse>, ApiError> {
    let conn = data_connection(&state)?;
    Ok(Json(
        sync::pull(&conn, query.since).map_err(ApiError::internal)?,
    ))
}

async fn list_meetings(
    State(state): State<AppState>,
    _device: AuthedDevice,
    Query(query): Query<DictationsQuery>,
) -> Result<Json<Vec<crate::meetings::MeetingView>>, ApiError> {
    let conn = data_connection(&state)?;
    let found = crate::meetings::list(
        &conn,
        query.q.as_deref(),
        query.before,
        query.limit.unwrap_or(50),
    )
    .map_err(ApiError::internal)?;
    Ok(Json(found))
}

async fn get_meeting(
    State(state): State<AppState>,
    _device: AuthedDevice,
    Path(id): Path<String>,
) -> Result<Json<crate::meetings::MeetingDetailView>, ApiError> {
    let conn = data_connection(&state)?;
    crate::meetings::detail(&conn, &id)
        .map_err(ApiError::internal)?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such meeting"))
}

#[derive(Deserialize)]
struct DictationsQuery {
    q: Option<String>,
    before: Option<i64>,
    limit: Option<usize>,
}

async fn list_dictations(
    State(state): State<AppState>,
    _device: AuthedDevice,
    Query(query): Query<DictationsQuery>,
) -> Result<Json<Vec<sync::DictationView>>, ApiError> {
    let conn = data_connection(&state)?;
    let found = sync::dictations(
        &conn,
        query.q.as_deref(),
        query.before,
        query.limit.unwrap_or(50),
    )
    .map_err(ApiError::internal)?;
    Ok(Json(found))
}

/// Builds the router for a hub with the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/init", post(init))
        .route("/api/unlock", post(unlock))
        .route("/api/lock", post(lock))
        .route("/api/devices", post(register_device).get(list_devices))
        .route("/api/devices/{id}", delete(revoke_device))
        .route("/api/me", get(me))
        .route("/api/sync/push", post(sync_push))
        .route("/api/sync/pull", get(sync_pull))
        .route("/api/dictations", get(list_dictations))
        .route("/api/meetings", get(list_meetings))
        .route("/api/meetings/{id}", get(get_meeting))
        // 500 records of up to 200 KB each would be 100 MB; real pushes are
        // far smaller. 16 MB bounds memory use per request.
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .merge(crate::web::router())
        .layer(axum::middleware::from_fn(crate::web::security_headers))
        .with_state(state)
}
