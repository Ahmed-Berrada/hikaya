#!/usr/bin/env bash
# End-to-end test of a phone recording (ADR 0004) on Linux, with the real
# app and a real hub: a "phone" device uploads a recording in chunks, the
# app claims it on its first sync, transcribes it, sends the meeting back,
# and the hub deletes the audio.
#
# Needs no audio devices. Uses throwaway data directories. Close any running
# Hikaya first, and build the debug app (bun run tauri dev, then quit it).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CACHE="${HIKAYA_E2E_CACHE:-$HOME/.cache/hikaya-e2e}"
MODEL_FILE="whisper-base-Q8_0.gguf"
MODEL_URL="https://huggingface.co/handy-computer/whisper-base-gguf/resolve/e0f69524f648720eca44c024d1d0dbb7027d1fa0/$MODEL_FILE"
MODEL_SHA="81c069428bc8a24551a8169cf31cf09bcfd9d4cf50389ae281323c9aa9648c81"
# John F. Kennedy's 1961 inaugural address, a US government work in the public domain.
SAMPLE_URL="https://github.com/ggml-org/whisper.cpp/raw/v1.7.4/samples/jfk.wav"
BIN="$REPO/src-tauri/target/debug/handy"
HUB_URL="http://127.0.0.1:${HIKAYA_E2E_HUB_PORT:-18793}"

say() { printf '== %s\n' "$*"; }
die() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
for tool in curl python3 sha256sum split; do
  command -v "$tool" >/dev/null || die "missing tool: $tool"
done

mkdir -p "$CACHE"
[[ -f "$CACHE/$MODEL_FILE" ]] || curl -fL --retry 3 -o "$CACHE/$MODEL_FILE" "$MODEL_URL"
echo "$MODEL_SHA  $CACHE/$MODEL_FILE" | sha256sum -c --quiet || die "model checksum mismatch"
[[ -f "$CACHE/jfk.wav" ]] || curl -fL --retry 3 -o "$CACHE/jfk.wav" "$SAMPLE_URL"
[[ -x "$BIN" ]] || die "build the debug app first: bun run tauri dev (then quit it)"
pgrep -x handy >/dev/null && die "Hikaya is already running; quit it first"
LIB_DIR="$(dirname "$(find "$REPO/src-tauri/target/debug/build" -name 'libtranscribe.so.*' -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/hikaya-phone.XXXXXX")"
DATA="$WORK/xdg/com.ahmedberrada.hikaya"
mkdir -p "$DATA/models"
ln -s "$CACHE/$MODEL_FILE" "$DATA/models/$MODEL_FILE"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  if [[ -n "${HIKAYA_E2E_KEEP:-}" ]]; then say "kept $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT
app() { XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" "$@"; }
start_app() {
  XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" --start-hidden --no-tray >>"$WORK/app.log" 2>&1 &
  APP_PID=$!
  PIDS+=("$APP_PID")
  for _ in $(seq 1 120); do grep -q "transcribe-cpp initialized" "$WORK/app.log" && return 0; sleep 0.5; done
  die "the app did not start"
}

say "preparing the app"
app --list-models >"$WORK/models.txt" 2>&1 || die "the app did not start"
MODEL_ID="$(sed -n "s/^ *✓ *\([^ ]*$MODEL_FILE\) .*/\1/p" "$WORK/models.txt" | head -1)"
[[ -n "$MODEL_ID" ]] || die "the app did not find $MODEL_FILE"
python3 - "$DATA" "$MODEL_ID" <<'PY'
import json, sys, pathlib
data, model = pathlib.Path(sys.argv[1]), sys.argv[2]
path = data / "settings_store.json"
store = json.loads(path.read_text())
store["settings"].update({"onboarding_completed": True, "paste_method": "none", "audio_feedback": False,
    "spoken_languages": ["en", "fr"], "selected_language": "en", "selected_model": model,
    "update_checks_enabled": False})
path.write_text(json.dumps(store))
PY
# A first start creates hikaya.db, where the hub connection is stored.
start_app
kill "$APP_PID"; wait "$APP_PID" 2>/dev/null || true

say "starting a hub"
(cd "$REPO/hub" && cargo build --quiet) || die "the hub did not build"
HIKAYA_HUB_DATA="$WORK/hub" HIKAYA_HUB_LISTEN="${HUB_URL#http://}" "$REPO/hub/target/debug/hikaya-hub" >"$WORK/hub.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 40); do curl -fs "$HUB_URL/api/status" >/dev/null && break; sleep 0.25; done
PASS="e2e passphrase $(date +%s)"
curl -fs -X POST "$HUB_URL/api/init" -H 'content-type: application/json' -d "{\"passphrase\":\"$PASS\"}" >/dev/null || die "hub init failed"
token() {
  curl -fs -X POST "$HUB_URL/api/devices" -H 'content-type: application/json' \
    -d "{\"passphrase\":\"$PASS\",\"name\":\"$1\"}" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])'
}
APP_TOKEN="$(token e2e-laptop)"
PHONE="$(token e2e-iphone)"
python3 - "$DATA/hikaya.db" "$HUB_URL" "$APP_TOKEN" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
for key, value in (("hub_url", sys.argv[2]), ("hub_token", sys.argv[3]), ("device_name", "e2e-laptop")):
    db.execute("INSERT INTO sync_settings (key, value) VALUES (?, ?)", (key, value))
db.commit()
PY

say "uploading a recording from the phone, in chunks"
auth=(-H "authorization: Bearer $PHONE")
ID="$(curl -fs -X POST "$HUB_URL/api/recordings" "${auth[@]}" -H 'content-type: application/json' \
  -d '{"title":"Phone e2e","mime":"audio/wav"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')"
split -b 100000 -d -a 3 "$CACHE/jfk.wav" "$WORK/chunk-"
count=0
for chunk in "$WORK"/chunk-*; do
  curl -fs -X PUT "$HUB_URL/api/recordings/$ID/chunks/$count" "${auth[@]}" \
    -H 'content-type: application/octet-stream' --data-binary "@$chunk" || die "chunk $count was refused"
  count=$((count + 1))
done
curl -fs -X POST "$HUB_URL/api/recordings/$ID/finish" "${auth[@]}" -H 'content-type: application/json' \
  -d "{\"chunks\":$count}" >/dev/null || die "the hub did not accept the finished recording"
echo "   recording $ID: $count chunks"

say "starting the app; its first sync should take the recording"
start_app
state=""
for _ in $(seq 1 180); do
  state="$(curl -fs "$HUB_URL/api/recordings/$ID" "${auth[@]}" | python3 -c 'import sys,json;r=json.load(sys.stdin);print(r["status"], r["chunks"], r["meeting_id"] or "")')"
  [[ "$state" == done* ]] && break
  sleep 1
done
[[ "$state" == done* ]] || { grep -iE "phone|hub|error" "$WORK/app.log" | tail -10; die "the recording was not completed (state: $state)"; }
read -r status chunks meeting <<<"$state"
[[ "$chunks" == 0 ]] || die "the hub kept the audio after completing"
echo "   recording is $status, audio deleted, meeting $meeting"

detail="$(curl -fs "$HUB_URL/api/meetings/$meeting" "${auth[@]}")"
echo "$detail" | python3 -c '
import sys, json
m = json.load(sys.stdin)
print("   %s from %s, %d line(s)" % (m["title"], m["device"], len(m["lines"])))
for l in m["lines"]:
    print("   %d %s %s" % (l["start_ms"], l["speaker"], l["text"]))
'
lower="$(printf '%s' "$detail" | tr '[:upper:]' '[:lower:]')"
[[ "$lower" == *'"speaker":"room"'* ]] || die "the lines are not labelled room"
[[ "$lower" == *"ask not what your country"* ]] || die "the transcript is missing the speech"
say "PASS: the phone's recording was transcribed by the app and is on the hub"
