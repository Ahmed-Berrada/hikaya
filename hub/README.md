# Hikaya hub

A small server you run on a machine you own. It keeps the complete archive of your dictations and
meetings, encrypted, and lets your devices sync through it. Design: [ADR 0002](../docs/adr/0002-linux-dev-and-hub-archive.md).

**Status:** the vault, device access and sync work. The desktop app does not sync yet; that,
the web viewer and background jobs are next (roadmap v0.3).

## How the vault works

- The first passphrase you send creates the vault. A key is derived from it with Argon2id and
  used as the SQLCipher key of `hub.db`.
- The key is only ever in memory. After a restart the hub is **locked**: it answers `423 Locked`
  to every data request until the passphrase is sent again. Clients keep their changes queued.
- `vault.json` holds the salt and the derivation settings. Nothing in it is secret.
- If the passphrase is lost, the archive cannot be recovered.
- A wrong passphrase makes the hub refuse further attempts for a while; the wait doubles each time.

## Run it

```bash
cd hub
HIKAYA_HUB_DATA=./data HIKAYA_HUB_LISTEN=127.0.0.1:8787 cargo run --release
```

The hub speaks plain HTTP. Listen on `127.0.0.1` or on a private-network address (for example a
Tailscale IP, with `tailscale serve` in front for HTTPS). Never expose it to the internet.

```bash
H=http://127.0.0.1:8787
curl $H/api/status
curl -X POST $H/api/init    -H 'content-type: application/json' -d '{"passphrase":"<at least 12 characters>"}'
curl -X POST $H/api/devices -H 'content-type: application/json' -d '{"passphrase":"…","name":"laptop"}'
curl $H/api/me -H 'authorization: Bearer <token from the previous call>'
curl -X POST $H/api/unlock  -H 'content-type: application/json' -d '{"passphrase":"…"}'   # after a restart
```

The device token is shown once. Only its SHA-256 is stored.

## API

| Route                       | Who may call it       | What it does                         |
| --------------------------- | --------------------- | ------------------------------------ |
| `GET /api/status`           | anyone on the network | lock state and version               |
| `POST /api/init`            | anyone, once          | creates the vault with a passphrase  |
| `POST /api/unlock`          | passphrase            | loads the key into memory            |
| `POST /api/lock`            | a device              | forgets the key                      |
| `POST /api/devices`         | passphrase            | registers a device, returns a token  |
| `GET /api/devices`          | a device              | lists devices                        |
| `DELETE /api/devices/{id}`  | passphrase            | revokes a device                     |
| `GET /api/me`               | a device              | the calling device                   |
| `POST /api/sync/push`       | a device              | sends dictations, rules, corrections |
| `GET /api/sync/pull?since=` | a device              | rule changes since a cursor          |
| `GET /api/dictations?q=`    | a device              | lists or searches the archive        |

### Sync rules

- Every record has an id chosen by the device, so sending it twice is harmless.
- A dictation can only be changed by the device that sent it; a newer `updated_at` replaces the text.
- Rules are shared: any device may change one, and the newest `updated_at` wins. A deleted rule
  stays as a tombstone so the deletion reaches every device.
- A push is applied all or nothing, and holds at most 500 records.
- Search matches whole words and word starts, ignoring case and accents.

Errors are JSON: `{"error": "<code>", "message": "<text>"}`.

## Develop

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

The first build compiles SQLCipher and OpenSSL from source and takes a few minutes.
