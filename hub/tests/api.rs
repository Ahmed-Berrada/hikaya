//! Drives the hub through its HTTP API, the way a client would.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use hikaya_hub::api::router;
use hikaya_hub::state::AppState;
use hikaya_hub::vault::{KdfCost, Vault};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const PASSPHRASE: &str = "correct horse battery staple";

struct Hub {
    dir: tempfile::TempDir,
    app: Router,
}

/// Cheap key derivation so the suite runs in milliseconds.
fn cost() -> KdfCost {
    KdfCost {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    }
}

impl Hub {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let app = router(AppState::new(Vault::with_cost(dir.path(), cost())));
        Self { dir, app }
    }

    /// The same data directory after a restart: the key is gone from memory.
    fn restarted(self) -> Self {
        let app = router(AppState::new(Vault::with_cost(self.dir.path(), cost())));
        Self { dir: self.dir, app }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }

    /// Sends raw bytes and returns the status and raw answer.
    async fn bytes(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: &'static [u8],
    ) -> (StatusCode, Option<String>, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/octet-stream")
            .body(Body::from(body))
            .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, mime, bytes.to_vec())
    }

    async fn raw(&self, path: &str) -> axum::response::Response {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("request");
        self.app.clone().oneshot(request).await.expect("response")
    }

    async fn state(&self) -> String {
        let (_, body) = self.call("GET", "/api/status", None, None).await;
        body["state"].as_str().expect("state").to_string()
    }

    async fn init(&self) {
        let (status, _) = self
            .call(
                "POST",
                "/api/init",
                None,
                Some(json!({ "passphrase": PASSPHRASE })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    async fn device(&self, name: &str) -> (i64, String) {
        let (status, body) = self
            .call(
                "POST",
                "/api/devices",
                None,
                Some(json!({ "passphrase": PASSPHRASE, "name": name })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        (
            body["device"]["id"].as_i64().expect("id"),
            body["token"].as_str().expect("token").to_string(),
        )
    }
}

#[tokio::test]
async fn a_new_hub_is_uninitialized_then_unlocked_after_init() {
    let hub = Hub::new();
    assert_eq!(hub.state().await, "uninitialized");
    hub.init().await;
    assert_eq!(hub.state().await, "unlocked");
}

#[tokio::test]
async fn init_refuses_a_short_passphrase_and_a_second_init() {
    let hub = Hub::new();
    let (status, body) = hub
        .call(
            "POST",
            "/api/init",
            None,
            Some(json!({ "passphrase": "short" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "weak_passphrase");
    assert_eq!(hub.state().await, "uninitialized");

    hub.init().await;
    let (status, body) = hub
        .call(
            "POST",
            "/api/init",
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "already_initialized");
}

#[tokio::test]
async fn after_a_restart_the_hub_is_locked_until_the_passphrase_is_sent() {
    let hub = Hub::new();
    hub.init().await;
    let (_, token) = hub.device("laptop").await;

    let hub = hub.restarted();
    assert_eq!(hub.state().await, "locked");

    // A valid token gets 423, not 401: the client must keep its queue and retry.
    let (status, body) = hub.call("GET", "/api/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(body["error"], "locked");

    let (status, _) = hub
        .call(
            "POST",
            "/api/unlock",
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hub.state().await, "unlocked");

    let (status, body) = hub.call("GET", "/api/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "laptop");
}

#[tokio::test]
async fn a_wrong_passphrase_is_refused_and_then_throttled() {
    let hub = Hub::new();
    hub.init().await;
    let hub = hub.restarted();

    let (status, body) = hub
        .call(
            "POST",
            "/api/unlock",
            None,
            Some(json!({ "passphrase": "not the passphrase" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "wrong_passphrase");
    assert_eq!(hub.state().await, "locked");

    // Even the right passphrase has to wait out the backoff.
    let (status, body) = hub
        .call(
            "POST",
            "/api/unlock",
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "throttled");
    assert_eq!(hub.state().await, "locked");
}

#[tokio::test]
async fn unlocking_an_uninitialized_hub_is_a_conflict() {
    let hub = Hub::new();
    let (status, body) = hub
        .call(
            "POST",
            "/api/unlock",
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "not_initialized");
}

#[tokio::test]
async fn devices_need_the_passphrase_to_register_and_a_token_to_call() {
    let hub = Hub::new();
    hub.init().await;

    let (status, body) = hub
        .call(
            "POST",
            "/api/devices",
            None,
            Some(json!({ "passphrase": "not the passphrase", "name": "intruder" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _) = hub.call("GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub.call("GET", "/api/me", Some("hky_made_up"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_registered_device_can_list_devices_and_a_revoked_one_cannot() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let (phone_id, phone) = hub.device("phone").await;
    assert_ne!(laptop, phone);
    assert!(laptop.starts_with("hky_"));

    let (status, body) = hub.call("GET", "/api/devices", Some(&laptop), None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|d| d["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["laptop", "phone"]);
    // Tokens and their hashes never come back out.
    assert!(!body.to_string().contains(&phone));
    assert!(!body.to_string().contains("token"));

    let (status, _) = hub
        .call(
            "DELETE",
            &format!("/api/devices/{phone_id}"),
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = hub.call("GET", "/api/me", Some(&phone), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub.call("GET", "/api/me", Some(&laptop), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = hub
        .call(
            "DELETE",
            "/api/devices/9999",
            None,
            Some(json!({ "passphrase": PASSPHRASE })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_device_can_lock_the_hub() {
    let hub = Hub::new();
    hub.init().await;
    let (_, token) = hub.device("laptop").await;

    let (status, _) = hub.call("POST", "/api/lock", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(hub.state().await, "unlocked");

    let (status, body) = hub.call("POST", "/api/lock", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "locked");
    let (status, _) = hub.call("GET", "/api/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::LOCKED);
}

#[tokio::test]
async fn device_names_are_validated() {
    let hub = Hub::new();
    hub.init().await;
    for name in ["", "   ", &"x".repeat(65)] {
        let (status, body) = hub
            .call(
                "POST",
                "/api/devices",
                None,
                Some(json!({ "passphrase": PASSPHRASE, "name": name })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"], "invalid_device");
    }
}

#[tokio::test]
async fn nothing_secret_is_readable_on_disk() {
    let hub = Hub::new();
    hub.init().await;
    let (_, token) = hub.device("laptop-de-ahmed").await;
    for entry in std::fs::read_dir(hub.dir.path()).expect("read dir") {
        let bytes = std::fs::read(entry.expect("entry").path()).expect("read file");
        for needle in [PASSPHRASE, token.as_str(), "laptop-de-ahmed"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "found {needle:?} in the clear"
            );
        }
    }
}

#[tokio::test]
async fn two_devices_sync_dictations_and_rules_through_the_hub() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let (_, phone) = hub.device("phone").await;

    let (status, body) = hub
        .call(
            "POST",
            "/api/sync/push",
            Some(&laptop),
            Some(json!({
                "dictations": [{ "id": "d1", "created_at": 1000, "updated_at": 1000,
                                 "text": "On se voit demain, and bring your laptop", "language": "fr" }],
                "rules": [{ "id": "r1", "pattern": "get hub", "replacement": "GitHub",
                            "enabled": true, "source": "learned", "updated_at": 1000 }],
                "substitutions": [{ "id": "s1", "from": "get hub", "to": "GitHub", "created_at": 900 }]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "dictations": 1, "rules": 1, "substitutions": 1, "meetings": 0, "words": 0, "snippets": 0 })
    );

    // The phone receives the rule.
    let (status, body) = hub
        .call("GET", "/api/sync/pull?since=0", Some(&phone), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rules"][0]["replacement"], "GitHub");
    let cursor = body["cursor"].as_i64().expect("cursor");
    let (_, body) = hub
        .call(
            "GET",
            &format!("/api/sync/pull?since={cursor}"),
            Some(&phone),
            None,
        )
        .await;
    assert_eq!(body["rules"], json!([]));

    // The archive is searchable from any device, accents ignored.
    let (status, body) = hub
        .call("GET", "/api/dictations?q=demain", Some(&phone), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["device"], "laptop");
    assert_eq!(body[0]["text"], "On se voit demain, and bring your laptop");
}

#[tokio::test]
async fn sync_needs_a_token_and_waits_while_locked() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let push =
        json!({ "dictations": [{ "id": "d1", "created_at": 1, "updated_at": 1, "text": "x" }] });

    let (status, _) = hub
        .call("POST", "/api/sync/push", None, Some(push.clone()))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let hub = hub.restarted();
    let (status, body) = hub
        .call("POST", "/api/sync/push", Some(&laptop), Some(push))
        .await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(body["error"], "locked");
}

#[tokio::test]
async fn sync_rejects_bad_records_and_other_devices_ids() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let (_, phone) = hub.device("phone").await;

    let (status, body) = hub
        .call(
            "POST",
            "/api/sync/push",
            Some(&laptop),
            Some(json!({ "dictations": [{ "id": "../x", "created_at": 1, "updated_at": 1, "text": "x" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_record");

    let mine =
        json!({ "dictations": [{ "id": "d1", "created_at": 1, "updated_at": 1, "text": "mine" }] });
    hub.call("POST", "/api/sync/push", Some(&laptop), Some(mine))
        .await;
    let theirs = json!({ "dictations": [{ "id": "d1", "created_at": 1, "updated_at": 9, "text": "theirs" }] });
    let (status, body) = hub
        .call("POST", "/api/sync/push", Some(&phone), Some(theirs))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "not_yours");

    let (status, _) = hub
        .call(
            "POST",
            "/api/sync/push",
            Some(&laptop),
            Some(json!({ "dictations": "nope" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn dictations_are_unreadable_on_disk() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let secret = "rendez-vous confidentiel avec le client Berrada";
    hub.call(
        "POST",
        "/api/sync/push",
        Some(&laptop),
        Some(json!({ "dictations": [{ "id": "d1", "created_at": 1, "updated_at": 1, "text": secret }] })),
    )
    .await;
    for entry in std::fs::read_dir(hub.dir.path()).expect("read dir") {
        let bytes = std::fs::read(entry.expect("entry").path()).expect("read");
        for needle in ["confidentiel", "Berrada"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "found {needle:?} in the clear"
            );
        }
    }
}

#[tokio::test]
async fn the_web_viewer_is_served_with_strict_headers() {
    let hub = Hub::new();
    for (path, kind) in [
        ("/", "text/html"),
        ("/app.js", "text/javascript"),
        ("/app.css", "text/css"),
        ("/icon.svg", "image/svg+xml"),
        ("/manifest.webmanifest", "application/manifest+json"),
    ] {
        let response = hub.raw(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let headers = response.headers();
        assert!(
            headers["content-type"]
                .to_str()
                .expect("ascii")
                .starts_with(kind),
            "{path}"
        );
        let csp = headers["content-security-policy"].to_str().expect("ascii");
        assert!(
            csp.contains("script-src 'self'") && !csp.contains("unsafe-inline"),
            "{path}"
        );
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
    }
    // The page has no inline script for the policy to allow.
    let body = hub
        .raw("/")
        .await
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(html.contains(r#"<script src="/app.js" defer></script>"#));
    assert_eq!(html.matches("<script").count(), 1);
}

#[tokio::test]
async fn api_answers_are_never_cached() {
    let hub = Hub::new();
    let response = hub.raw("/api/status").await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(hub.raw("/").await.headers().get("cache-control").is_none());
    assert_eq!(hub.raw("/nope").await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_meeting_pushed_by_one_device_is_readable_from_another() {
    let hub = Hub::new();
    hub.init().await;
    let (_, laptop) = hub.device("laptop").await;
    let (_, phone) = hub.device("phone").await;
    let (status, body) = hub
        .call(
            "POST",
            "/api/sync/push",
            Some(&laptop),
            Some(json!({ "meetings": [{
                "id": "m1", "title": "Point produit", "started_at": 1000, "duration_ms": 60000,
                "updated_at": 1000, "summary": "## TL;DR\nDémo jeudi.",
                "lines": [{ "start_ms": 0, "end_ms": 30000, "speaker": "others", "text": "On décale la démo ?" }]
            }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["meetings"], 1);

    let (status, body) = hub
        .call("GET", "/api/meetings?q=demo", Some(&phone), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["title"], "Point produit");
    assert_eq!(body[0]["device"], "laptop");
    assert_eq!(body[0]["line_count"], 1);

    let (status, body) = hub
        .call("GET", "/api/meetings/m1", Some(&phone), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["summary"], "## TL;DR\nDémo jeudi.");
    assert_eq!(body["lines"][0]["speaker"], "others");

    let (status, _) = hub
        .call("GET", "/api/meetings/nope", Some(&phone), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = hub.call("GET", "/api/meetings", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_phone_recording_is_transcribed_by_a_computer_through_the_hub() {
    let hub = Hub::new();
    hub.init().await;
    let (_, phone) = hub.device("iphone").await;
    let (_, laptop) = hub.device("laptop").await;

    let (status, body) = hub
        .call(
            "POST",
            "/api/recordings",
            Some(&phone),
            Some(json!({ "title": "Café avec Sam", "mime": "audio/mp4" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().expect("id").to_string();

    for (index, chunk) in [(0, &b"head"[..]), (1, &b"tail"[..])] {
        let (status, _, _) = hub
            .bytes(
                "PUT",
                &format!("/api/recordings/{id}/chunks/{index}"),
                &phone,
                chunk,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, body) = hub
        .call(
            "POST",
            &format!("/api/recordings/{id}/finish"),
            Some(&phone),
            Some(json!({ "chunks": 3 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["message"], "chunk 2 is missing");
    let (status, body) = hub
        .call(
            "POST",
            &format!("/api/recordings/{id}/finish"),
            Some(&phone),
            Some(json!({ "chunks": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");

    let (status, body) = hub
        .call("POST", "/api/recordings/claim", Some(&laptop), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], id.as_str());
    let (status, _) = hub
        .call("POST", "/api/recordings/claim", Some(&laptop), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "nothing else waits");

    let (status, mime, audio) = hub
        .bytes("GET", &format!("/api/recordings/{id}/audio"), &laptop, b"")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mime.as_deref(), Some("audio/mp4"));
    assert_eq!(audio, b"headtail");
    let (status, _, _) = hub
        .bytes("GET", &format!("/api/recordings/{id}/audio"), &phone, b"")
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "only the claimer downloads");

    let (status, body) = hub
        .call(
            "POST",
            "/api/sync/push",
            Some(&laptop),
            Some(json!({ "meetings": [{
                "id": "m-phone", "title": "Café avec Sam", "started_at": 1000,
                "duration_ms": 30000, "updated_at": 1000, "summary": null,
                "lines": [{ "start_ms": 0, "end_ms": 30000, "speaker": "room", "text": "On se voit jeudi." }]
            }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = hub
        .call(
            "POST",
            &format!("/api/recordings/{id}/complete"),
            Some(&laptop),
            Some(json!({ "meeting_id": "m-phone" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "done");

    let (status, body) = hub.call("GET", "/api/recordings", Some(&phone), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["meeting_id"], "m-phone");
    assert_eq!(body[0]["chunks"], 0, "the audio is gone");
    let (_, body) = hub
        .call("GET", "/api/meetings/m-phone", Some(&phone), None)
        .await;
    assert_eq!(body["lines"][0]["speaker"], "room");
}

#[tokio::test]
async fn recordings_need_a_token_and_a_known_audio_type() {
    let hub = Hub::new();
    hub.init().await;
    let (_, phone) = hub.device("iphone").await;
    let (status, _) = hub.call("GET", "/api/recordings", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub
        .call(
            "POST",
            "/api/recordings",
            Some(&phone),
            Some(json!({ "title": "x", "mime": "text/html" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = hub
        .bytes("PUT", "/api/recordings/nope/chunks/0", &phone, b"x")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_shortcut_uploads_a_voice_note_and_polls_for_its_text() {
    let hub = Hub::new();
    hub.init().await;
    let (_, phone) = hub.device("shortcut").await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/recordings/upload?title=Courses")
        .header("authorization", format!("Bearer {phone}"))
        .header("content-type", "audio/mp4")
        .body(Body::from(&b"m4a bytes"[..]))
        .expect("request");
    let response = hub.app.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let created: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(created["title"], "Courses");
    assert_eq!(created["status"], "pending");
    let id = created["id"].as_str().expect("id");

    let (status, body) = hub
        .call(
            "GET",
            &format!("/api/recordings/{id}/text"),
            Some(&phone),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "pending");
    assert_eq!(body["text"], Value::Null);
}

#[tokio::test]
async fn an_audio_body_larger_than_a_chunk_is_refused_before_it_is_stored() {
    let hub = Hub::new();
    hub.init().await;
    let (_, phone) = hub.device("iphone").await;
    let (_, body) = hub
        .call(
            "POST",
            "/api/recordings",
            Some(&phone),
            Some(json!({ "title": "x", "mime": "audio/mp4" })),
        )
        .await;
    let id = body["id"].as_str().expect("id").to_string();
    let big: &'static [u8] = Box::leak(vec![0u8; 9 * 1024 * 1024].into_boxed_slice());
    let (status, _, _) = hub
        .bytes(
            "PUT",
            &format!("/api/recordings/{id}/chunks/0"),
            &phone,
            big,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
