# 0002: Develop on Linux, hub is the encrypted archive

- Status: accepted (supersedes the development-platform and hub-optional parts of
  [0001](0001-device-topology.md))
- Date: 2026-11-16

## Context

ADR 0001 assumed development on the Windows GPU laptop and treated the home server as an optional
hub added late (v0.4). Two things changed:

- All development now happens on the Linux home server. It cannot build or run Windows binaries.
- The home server must hold the complete archive of dictations, recordings and transcripts. It is
  an always-on machine with an unencrypted system disk that reboots unattended.

## Decision

- **Development and automated testing run on Linux.** Windows installers come from CI on each
  release and are checked by hand with [RELEASE_CHECKLIST.md](../hikaya/RELEASE_CHECKLIST.md).
- **Logic lives outside platform code.** Text rules, correction mining, prompt building and the
  sync model go in `crates/hikaya-core` (pure Rust, no Tauri). Platform-specific code (hotkeys,
  paste, audio capture) stays thin. The app only adds glue in `src-tauri/src/hikaya/`.
- **No root Cargo workspace.** `src-tauri` and `hub` depend on `crates/hikaya-core` by path, so
  upstream build paths and workflows are untouched.
- **Clients keep a local copy and sync to the hub.** A client works offline and pushes from an
  outbox when the hub is reachable. Rows carry a ULID and a device id; the dictionary and rules
  merge last-writer-wins per row.
- **Hub data is encrypted at rest**: SQLCipher for the database and `age` for audio files. The
  data key is wrapped by a passphrase and held only in memory. After a restart the hub is
  **locked** until the passphrase is entered over the private network; clients queue meanwhile.
- **Heavy inference stays on GPU clients.** The hub transcribes or summarizes only as a
  background fallback when no GPU client did it.
- **The hub moves ahead of meetings** in the roadmap, so meeting data is archived from day one.
- **Versioning restarts at 0.1.0** with Hikaya's own bundle identifier, so Hikaya installs beside
  Handy and never shares its data.

## Consequences

- Windows-only code has a slow feedback loop (a CI build plus a manual install). It must stay small.
- After an unattended reboot the hub does nothing until it is unlocked. The app has to show that
  state clearly.
- A stolen server exposes no transcripts or audio, as long as it was powered off or locked.
- Backups contain ciphertext only, so they can be copied off-site safely.
- Losing the passphrase loses the archive. Each client's local copy is the only fallback.
- Recording other people can require their consent (in France, Code pénal art. 226-1). The meeting
  recorder shows a visible indicator and a reminder; obtaining consent is the user's responsibility.
