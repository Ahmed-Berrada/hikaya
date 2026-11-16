//! The web viewer: a small page to unlock the hub and search the archive
//! from any browser on the private network, an iPhone included.
//!
//! The files are compiled into the binary, so the hub stays one executable.

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const ICON: &str = include_str!("../web/icon.svg");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");

/// Only the hub's own files may run or be fetched; no inline script, no
/// framing. Dictation text is untrusted, so this is the second line of
/// defence behind the page inserting it as text only.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
connect-src 'self'; manifest-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

fn file(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// Routes for the page and its assets.
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/",
            get(|| async { file("text/html; charset=utf-8", INDEX) }),
        )
        .route(
            "/app.js",
            get(|| async { file("text/javascript; charset=utf-8", APP_JS) }),
        )
        .route(
            "/app.css",
            get(|| async { file("text/css; charset=utf-8", APP_CSS) }),
        )
        .route("/icon.svg", get(|| async { file("image/svg+xml", ICON) }))
        .route(
            "/manifest.webmanifest",
            get(|| async { file("application/manifest+json", MANIFEST) }),
        )
}

/// Adds security headers to every response, and forbids caching API answers
/// (they contain the archive).
pub async fn security_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}
