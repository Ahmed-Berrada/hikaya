use anyhow::{Context, Result};
use hikaya_hub::{api, state::AppState, vault::Vault};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Settings come from the environment so the same binary runs in Docker and
/// on a developer machine.
///
/// - `HIKAYA_HUB_DATA`: directory for the vault (default `./data`).
/// - `HIKAYA_HUB_LISTEN`: address to listen on (default `127.0.0.1:8787`).
///   Bind it to a private-network address only; the hub has no TLS of its own.
/// - `HIKAYA_HUB_SNAPSHOT_HOURS`: hours between backup snapshots (default 6).
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hikaya_hub=info".into()),
        )
        .init();

    let data = PathBuf::from(std::env::var("HIKAYA_HUB_DATA").unwrap_or_else(|_| "data".into()));
    let listen: SocketAddr = std::env::var("HIKAYA_HUB_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()
        .context("HIKAYA_HUB_LISTEN is not a valid address")?;

    let state = AppState::new(Vault::new(&data));
    tracing::info!(
        data = %data.display(),
        state = ?state.lock_state(),
        "Hikaya hub {} listening on http://{listen}",
        env!("CARGO_PKG_VERSION")
    );

    // A consistent, encrypted snapshot for the nightly backup, every few hours
    // while unlocked. See hub/README.md, "Backups".
    let every: u64 = std::env::var("HIKAYA_HUB_SNAPSHOT_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let snapshots = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(every.max(1) * 3600));
        tick.tick().await; // the first tick is immediate; skip it
        loop {
            tick.tick().await;
            let worker = snapshots.clone();
            let stamp = hikaya_hub::snapshot::stamp(std::time::SystemTime::now());
            match tokio::task::spawn_blocking(move || worker.snapshot(&stamp)).await {
                Ok(Ok(Some(path))) => tracing::info!(path = %path.display(), "snapshot written"),
                Ok(Ok(None)) => tracing::info!("hub locked; snapshot skipped"),
                Ok(Err(e)) => tracing::error!("snapshot failed: {e:#}"),
                Err(e) => tracing::error!("snapshot task failed: {e}"),
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serving")
}
