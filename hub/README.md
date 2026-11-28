# Hikaya hub

A small server you run on a machine you own. It keeps the complete archive of your dictations and
meetings, encrypted, and lets your devices sync through it. Design: [ADR 0002](../docs/adr/0002-linux-dev-and-hub-archive.md).

**Status:** the vault, device access, sync, snapshots, Docker packaging and the web viewer work.
Background jobs are next.

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

| Route                       | Who may call it       | What it does                                                 |
| --------------------------- | --------------------- | ------------------------------------------------------------ |
| `GET /api/status`           | anyone on the network | lock state and version                                       |
| `POST /api/init`            | anyone, once          | creates the vault with a passphrase                          |
| `POST /api/unlock`          | passphrase            | loads the key into memory                                    |
| `POST /api/lock`            | a device              | forgets the key                                              |
| `POST /api/devices`         | passphrase            | registers a device, returns a token                          |
| `GET /api/devices`          | a device              | lists devices                                                |
| `DELETE /api/devices/{id}`  | passphrase            | revokes a device                                             |
| `GET /api/me`               | a device              | the calling device                                           |
| `POST /api/sync/push`       | a device              | sends dictations, rules, corrections, meetings, custom words |
| `GET /api/sync/pull?since=` | a device              | rule and custom-word changes since a cursor                  |
| `GET /api/dictations?q=`    | a device              | lists or searches the archive                                |
| `GET /api/meetings?q=`      | a device              | lists or searches meetings                                   |
| `GET /api/meetings/{id}`    | a device              | one meeting with its transcript                              |

### Sync rules

- Every record has an id chosen by the device, so sending it twice is harmless.
- A dictation can only be changed by the device that sent it; a newer `updated_at` replaces the text.
- Rules are shared: any device may change one, and the newest `updated_at` wins. A deleted rule
  stays as a tombstone so the deletion reaches every device.
- Custom words work the same way, with the word itself as the id: adding a word on one computer
  adds it on the others, and removing it removes it everywhere. Words are case-sensitive. Snippets
  (a spoken cue and the text it types) are shared the same way, by id. After
  switching a computer to a new hub, a word removed long ago may come back from another computer
  that still had it: remove it again.
- A push is applied all or nothing, and holds at most 500 records.
- Search matches whole words and word starts, ignoring case and accents.

Errors are JSON: `{"error": "<code>", "message": "<text>"}`.

## Deploy with Docker

On the machine that will hold the archive (Linux with Docker):

```bash
mkdir -p ~/hikaya-data && chmod 700 ~/hikaya-data
HIKAYA_HUB_DATA_DIR=$HOME/hikaya-data HIKAYA_HUB_UID=$(id -u) HIKAYA_HUB_GID=$(id -g) \
  docker compose -f hub/compose.yaml up -d --build
curl http://127.0.0.1:8787/api/status          # {"state":"uninitialized",...}
```

The container runs as your user (so your backups can read the files), with a read-only filesystem
and no extra privileges, and publishes the port on `127.0.0.1` only.

To reach it from your other devices, put it on your Tailscale network with HTTPS:

```bash
sudo tailscale serve --bg --https=443 http://127.0.0.1:8787
tailscale serve status                          # shows https://<machine>.<tailnet>.ts.net
```

Then, in the Hikaya app: **Sync → Hub address** `https://<machine>.<tailnet>.ts.net`, a device
name, and a passphrase. On a new hub that passphrase creates the vault. Until then, any device on
your tailnet could create it, so connect your first device right after starting the hub.

After the machine reboots, the hub is locked. Unlock it from **Sync** in the app on any device.

## Web viewer (and iPhone)

Open the hub's address in a browser on your tailnet, for example on your iPhone:
`https://<machine>.<tailnet>.ts.net`. Pair the browser once with the passphrase. You can then
search every dictation, read meetings with their summaries, and unlock the hub after a reboot without the desktop app. In Safari, **Share
→ Add to Home Screen** makes it open like an app.

**Record** records with the phone's microphone, for in-person meetings and voice notes. Keep the
page open and the screen on until you press Stop: Safari pauses recording in the background. Audio
is uploaded every few seconds and kept encrypted on the hub. The next time the Hikaya app syncs on
your computer, it transcribes the recording, summarizes it if a provider is set up, and the
meeting appears under Meetings everywhere; the hub then deletes the audio. Recording needs HTTPS
(`tailscale serve`), as browsers only allow the microphone on secure pages.

The browser keeps only its device token; revoke it like any other device. The page loads no
outside resources and runs under a strict Content-Security-Policy.

## Summaries on the hub (optional)

Your computer summarizes meetings with its own AI provider. When a meeting reaches the hub without
a summary (no provider set up, or nobody pressed Summarize), the hub can write one with a local
[Ollama](https://ollama.com) model after 30 minutes. It is shown with the note "written by the
hub". If the computer sends its own summary later, that one is shown instead.

With Ollama in a Docker container named `ollama` on the same machine:

```bash
docker network create hikaya-ai
docker network connect hikaya-ai ollama
docker exec ollama ollama pull qwen3:4b-instruct
HIKAYA_HUB_DATA_DIR=$HOME/hikaya-data docker compose -f hub/compose.yaml -f hub/compose.ollama.yaml up -d --build
```

Without Docker, set `HIKAYA_HUB_OLLAMA_URL` (for example `http://127.0.0.1:11434`). Other settings:
`HIKAYA_HUB_OLLAMA_MODEL` (default `qwen3:4b-instruct`) and `HIKAYA_HUB_SUMMARY_DELAY_MINUTES`
(default 30). On a laptop CPU a 4B model takes about five minutes per meeting; the hub does one
at a time, checks every five minutes, and waits a day before retrying a failed one. The transcript
never leaves the machine.

## Backups

While unlocked, the hub writes a consistent snapshot of its database to `snapshots/` every
`HIKAYA_HUB_SNAPSHOT_HOURS` (default 6) and keeps the newest three. Snapshots are encrypted with the
same key as the live database. Back up `vault.json` and `snapshots/`; the live `hub.db` can be
skipped. To restore, copy a snapshot to `hub.db` next to `vault.json` and unlock with the passphrase.

## Develop

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

The first build compiles SQLCipher and OpenSSL from source and takes a few minutes.
