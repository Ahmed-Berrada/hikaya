#!/usr/bin/env bash
# End-to-end dictation test on Linux with PipeWire, no human needed.
#
# Plays a recorded sentence into a virtual microphone while the real app
# records, then checks the text the app saved to its history. Along the way it
# proves that capture, VAD, speech-to-text, Hikaya's filler/rule stage and
# history all work together.
#
#   bun run tauri dev      # once, to build src-tauri/target/debug/handy
#   scripts/hikaya/e2e-linux.sh
#
# With HIKAYA_E2E_HUB=1 it also starts a real hub (built from hub/), connects
# the app to it, and checks that the dictation arrives on the hub.
#
# Safety: the app runs with a throwaway data directory (XDG_DATA_HOME) and
# with pasting switched off, so nothing is typed into the desktop and no real
# history or settings are touched. The default microphone is switched to the
# virtual one for the duration of the test and restored afterwards. Close any
# running Hikaya first: the test talks to its own instance by app id.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CACHE="${HIKAYA_E2E_CACHE:-$HOME/.cache/hikaya-e2e}"
MODEL_FILE="whisper-base-Q8_0.gguf"
MODEL_URL="https://huggingface.co/handy-computer/whisper-base-gguf/resolve/e0f69524f648720eca44c024d1d0dbb7027d1fa0/$MODEL_FILE"
MODEL_SHA="81c069428bc8a24551a8169cf31cf09bcfd9d4cf50389ae281323c9aa9648c81"
# John F. Kennedy's 1961 inaugural address, a US government work in the public domain.
SAMPLE_URL="https://github.com/ggml-org/whisper.cpp/raw/v1.7.4/samples/jfk.wav"
BIN="$REPO/src-tauri/target/debug/handy"

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"

say() { printf '== %s\n' "$*"; }
die() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
for tool in pw-cli pw-loopback pw-play wpctl curl python3 sha256sum; do
  command -v "$tool" >/dev/null || die "missing tool: $tool"
done

mkdir -p "$CACHE"
[[ -f "$CACHE/$MODEL_FILE" ]] || { say "downloading $MODEL_FILE (85 MB)"; curl -fL --retry 3 -o "$CACHE/$MODEL_FILE.part" "$MODEL_URL" && mv "$CACHE/$MODEL_FILE.part" "$CACHE/$MODEL_FILE"; }
echo "$MODEL_SHA  $CACHE/$MODEL_FILE" | sha256sum -c --quiet || die "model checksum mismatch"
[[ -f "$CACHE/jfk.wav" ]] || { say "downloading the speech sample"; curl -fL --retry 3 -o "$CACHE/jfk.wav" "$SAMPLE_URL"; }

[[ -x "$BIN" ]] || die "build the debug app first: bun run tauri dev (then quit it)"
pgrep -x handy >/dev/null && die "Hikaya is already running; quit it first"
LIB_DIR="$(dirname "$(find "$REPO/src-tauri/target/debug/build" -name 'libtranscribe.so.*' -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)")"
[[ -d "$LIB_DIR" ]] || die "libtranscribe not found under target/debug/build"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/hikaya-e2e.XXXXXX")"
DATA="$WORK/xdg/com.ahmedberrada.hikaya"
mkdir -p "$DATA/models"
ln -s "$CACHE/$MODEL_FILE" "$DATA/models/$MODEL_FILE"
app() { XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" "$@"; }

node_id() { pw-cli ls Node 2>/dev/null | awk -v name="$1" '/^\tid /{id=$2; sub(",","",id)} index($0, "node.name = \"" name "\""){print id; exit}'; }
previous_source="$(wpctl inspect @DEFAULT_AUDIO_SOURCE@ 2>/dev/null | sed -n 's/.*node.name = "\(.*\)"/\1/p' | head -1 || true)"
APP_PID=""; MIC_PID=""
cleanup() {
  [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null || true
  [[ -n "$MIC_PID" ]] && kill "$MIC_PID" 2>/dev/null || true
  [[ -n "${HUB_PID:-}" ]] && kill "$HUB_PID" 2>/dev/null || true
  if [[ -n "$previous_source" ]]; then
    id="$(node_id "$previous_source" || true)"
    [[ -n "${id:-}" ]] && wpctl set-default "$id" 2>/dev/null || true
  fi
  if [[ -n "${HIKAYA_E2E_KEEP:-}" ]]; then say "kept $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

say "creating a virtual microphone"
# A loopback pair: what is played into the sink comes out of the source.
pw-loopback \
  --capture-props='media.class=Audio/Sink node.name=hikaya-e2e-sink node.description="Hikaya E2E input"' \
  --playback-props='media.class=Audio/Source node.name=hikaya-e2e-mic node.description="Hikaya E2E microphone"' \
  >/dev/null 2>&1 &
MIC_PID=$!
mic_id=""
for _ in $(seq 1 20); do mic_id="$(node_id hikaya-e2e-mic || true)"; [[ -n "$mic_id" ]] && break; sleep 0.25; done
[[ -n "$mic_id" ]] || die "the virtual microphone did not appear"
wpctl set-default "$mic_id" || die "could not make the virtual microphone the default"

say "first start, to create the data folder and list the models"
app --list-models >"$WORK/models.txt" 2>&1 || { tail -5 "$WORK/models.txt"; die "the app did not start"; }
MODEL_ID="$(sed -n "s/^ *✓ *\([^ ]*$MODEL_FILE\) .*/\1/p" "$WORK/models.txt" | head -1)"
[[ -n "$MODEL_ID" ]] || die "the app did not find $MODEL_FILE in its models folder"

say "writing test settings"
python3 - "$DATA" "$MODEL_ID" <<'PY'
import json, sys, pathlib
data, model = pathlib.Path(sys.argv[1]), sys.argv[2]
path = data / "settings_store.json"
# A headless run can exit before the store's autosave writes the defaults.
# The app fills in whatever is missing, so a partial store is enough.
store = json.loads(path.read_text()) if path.exists() else {"settings": {}}
store["settings"].update({
    "onboarding_completed": True,
    "paste_method": "none",          # never type into the desktop
    "audio_feedback": False,
    "spoken_languages": ["en", "fr"],
    "selected_language": "en",
    "selected_model": model,
    "update_checks_enabled": False,
    "custom_words": ["Hikaya"],      # must reach the hub when one is connected
})
path.write_text(json.dumps(store))
PY

start_app() {
  # Started directly, not through app(), so $! is the app and cleanup can stop it.
  XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" --start-hidden --no-tray >>"$WORK/app.log" 2>&1 &
  APP_PID=$!
  for _ in $(seq 1 120); do
    grep -q "transcribe-cpp initialized" "$WORK/app.log" && return 0
    kill -0 "$APP_PID" 2>/dev/null || { tail -20 "$WORK/app.log"; die "the app exited during startup"; }
    sleep 0.5
  done
  die "the app did not finish starting"
}
stop_app() { kill "$APP_PID" 2>/dev/null || true; wait "$APP_PID" 2>/dev/null || true; APP_PID=""; : >"$WORK/app.log"; }

say "starting the app once so it creates its own databases"
start_app
stop_app

HUB_PID=""
if [[ -n "${HIKAYA_E2E_HUB:-}" ]]; then
  say "building and starting a hub"
  (cd "$REPO/hub" && cargo build --quiet) || die "the hub did not build"
  HUB_URL="http://127.0.0.1:${HIKAYA_E2E_HUB_PORT:-18787}"
  HIKAYA_HUB_DATA="$WORK/hub" HIKAYA_HUB_LISTEN="${HUB_URL#http://}" "$REPO/hub/target/debug/hikaya-hub" >"$WORK/hub.log" 2>&1 &
  HUB_PID=$!
  for _ in $(seq 1 40); do curl -fs "$HUB_URL/api/status" >/dev/null && break; sleep 0.25; done
  PASS="e2e passphrase $(date +%s)"
  curl -fs -X POST "$HUB_URL/api/init" -H 'content-type: application/json' -d "{\"passphrase\":\"$PASS\"}" >/dev/null || die "hub init failed"
  HUB_TOKEN="$(curl -fs -X POST "$HUB_URL/api/devices" -H 'content-type: application/json' \
    -d "{\"passphrase\":\"$PASS\",\"name\":\"e2e-laptop\"}" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')"
  [[ -n "$HUB_TOKEN" ]] || die "could not register a device on the hub"
  # A rule made on "another device", which the app must pull and apply.
  READER="$(curl -fs -X POST "$HUB_URL/api/devices" -H 'content-type: application/json' \
    -d "{\"passphrase\":\"$PASS\",\"name\":\"e2e-phone\"}" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')"
  curl -fs -X POST "$HUB_URL/api/sync/push" -H "authorization: Bearer $READER" -H 'content-type: application/json' \
    -d "{\"rules\":[{\"id\":\"phone-rule-1\",\"pattern\":\"your country\",\"replacement\":\"your community\",\"enabled\":true,\"source\":\"manual\",\"updated_at\":$(date +%s%3N)}]}" >/dev/null \
    || die "could not add the remote rule"
  # A custom word added on "another device", which must land in the app's settings.
  curl -fs -X POST "$HUB_URL/api/sync/push" -H "authorization: Bearer $READER" -H 'content-type: application/json' \
    -d "{\"words\":[{\"word\":\"Kubernetes\",\"updated_at\":$(date +%s%3N)}]}" >/dev/null \
    || die "could not add the remote custom word"
fi

say "adding one local replacement rule"
python3 - "$DATA/hikaya.db" "${HUB_URL:-}" "${HUB_TOKEN:-}" <<'PY'
import sqlite3, sys, time
db = sqlite3.connect(sys.argv[1])
now = int(time.time() * 1000)
db.execute(
    "INSERT INTO rules (pattern, replacement, created_at, uid, updated_at) VALUES (?, ?, ?, lower(hex(randomblob(16))), ?)",
    ("fellow americans", "fellow Canadians", now // 1000, now),
)
url, token = sys.argv[2], sys.argv[3]
if url:
    for key, value in (("hub_url", url), ("hub_token", token), ("device_name", "e2e-laptop")):
        db.execute("INSERT INTO sync_settings (key, value) VALUES (?, ?)", (key, value))
db.commit()
PY

say "starting the app"
start_app
# Long enough for the first background sync to pull the hub's rules.
sleep "${HIKAYA_E2E_HUB:+8}${HIKAYA_E2E_HUB:-3}"

say "dictating the sample"
app --toggle-transcription >/dev/null 2>&1
sleep 1.5
timeout 60 pw-play --target hikaya-e2e-sink "$CACHE/jfk.wav" || die "could not play the sample"
sleep 1
app --toggle-transcription >/dev/null 2>&1

say "waiting for the transcription"
text=""
for _ in $(seq 1 120); do
  text="$(python3 - "$DATA/history.db" <<'PY' 2>/dev/null || true
import sqlite3, sys
row = sqlite3.connect(sys.argv[1]).execute("SELECT transcription_text FROM transcription_history ORDER BY id DESC LIMIT 1").fetchone()
print(row[0] if row else "")
PY
)"
  [[ -n "$text" ]] && break
  sleep 1
done
[[ -n "$text" ]] || { grep -iE "error|warn|silen|recording" "$WORK/app.log" | tail -8; die "no history entry after 120 s"; }
echo "   heard: $text"

lower="$(printf '%s' "$text" | tr '[:upper:]' '[:lower:]')"
# "country" may have been replaced by the hub rule, so check the words around it.
[[ "$lower" == *"ask not what your"*"can do for you"* ]] || die "the sentence was not transcribed"
[[ "$text" == *"fellow Canadians"* ]] || die "the replacement rule was not applied"
[[ "$lower" != *"fellow americans"* ]] || die "the original words are still there"
say "PASS: capture, VAD, speech-to-text, rules and history all work"

if [[ -n "${HIKAYA_E2E_HUB:-}" ]]; then
  [[ "$lower" == *"your community"* ]] || die "the rule made on another device was not pulled and applied"
  say "waiting for the dictation to reach the hub"
  found=""
  for _ in $(seq 1 30); do
    found="$(curl -fs "$HUB_URL/api/dictations?q=canadians" -H "authorization: Bearer $READER" \
      | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d[0]["text"] if d else "")')"
    [[ -n "$found" ]] && break
    sleep 1
  done
  [[ -n "$found" ]] || { grep -i "hub" "$WORK/app.log" | tail -5; die "the dictation never reached the hub"; }
  echo "   hub has: $found"
  say "PASS: the dictation was synced to the hub and a rule from another device was applied"

  say "checking custom words both ways"
  words="$(curl -fs "$HUB_URL/api/sync/pull?since=0" -H "authorization: Bearer $READER" \
    | python3 -c 'import sys,json;print(" ".join(sorted(w["word"] for w in json.load(sys.stdin)["words"] if not w.get("deleted"))))')"
  [[ "$words" == "Hikaya Kubernetes" ]] || die "the hub has custom words [$words], expected [Hikaya Kubernetes]"
  local_words="$(python3 -c 'import sys,json;print(" ".join(sorted(json.load(open(sys.argv[1]))["settings"]["custom_words"])))' "$DATA/settings_store.json")"
  [[ "$local_words" == "Hikaya Kubernetes" ]] || die "the app has custom words [$local_words], expected [Hikaya Kubernetes]"
  say "PASS: a custom word reached the hub and one from another device reached the app's settings"
fi
