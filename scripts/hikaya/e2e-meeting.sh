#!/usr/bin/env bash
# End-to-end meeting test on Linux with PipeWire, no human needed.
#
# Creates a virtual speaker (its monitor is the "Others" track) and a virtual
# microphone ("You"), starts a meeting in the real app, plays a recording
# into the speaker, then 30 s later into the microphone, stops the meeting,
# and checks that both tracks were saved and transcribed under the right
# speaker.
#
#   bun run tauri dev      # once, to build src-tauri/target/debug/handy
#   scripts/hikaya/e2e-meeting.sh
#
# Uses a throwaway data directory; the default speaker and microphone are
# switched to the virtual ones for the test and restored afterwards. Close
# any running Hikaya first.
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
[[ -f "$CACHE/$MODEL_FILE" ]] || curl -fL --retry 3 -o "$CACHE/$MODEL_FILE" "$MODEL_URL"
echo "$MODEL_SHA  $CACHE/$MODEL_FILE" | sha256sum -c --quiet || die "model checksum mismatch"
[[ -f "$CACHE/jfk.wav" ]] || curl -fL --retry 3 -o "$CACHE/jfk.wav" "$SAMPLE_URL"
[[ -x "$BIN" ]] || die "build the debug app first: bun run tauri dev (then quit it)"
pgrep -x handy >/dev/null && die "Hikaya is already running; quit it first"
LIB_DIR="$(dirname "$(find "$REPO/src-tauri/target/debug/build" -name 'libtranscribe.so.*' -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/hikaya-meeting.XXXXXX")"
DATA="$WORK/xdg/com.ahmedberrada.hikaya"
mkdir -p "$DATA/models"
ln -s "$CACHE/$MODEL_FILE" "$DATA/models/$MODEL_FILE"
app() { XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" "$@"; }
node_id() { pw-cli ls Node 2>/dev/null | awk -v name="$1" '/^\tid /{id=$2; sub(",","",id)} index($0, "node.name = \"" name "\""){print id; exit}'; }
current() { wpctl inspect "$1" 2>/dev/null | sed -n 's/.*node.name = "\(.*\)"/\1/p' | head -1 || true; }

previous_source="$(current @DEFAULT_AUDIO_SOURCE@)"
previous_sink="$(current @DEFAULT_AUDIO_SINK@)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  for name in "$previous_source" "$previous_sink"; do
    [[ -n "$name" ]] || continue
    id="$(node_id "$name" || true)"
    [[ -n "${id:-}" ]] && wpctl set-default "$id" 2>/dev/null || true
  done
  if [[ -n "${HIKAYA_E2E_KEEP:-}" ]]; then say "kept $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

say "creating a virtual microphone and a virtual speaker"
# Microphone: what is played into hikaya-e2e-mic-in comes out of hikaya-e2e-mic.
pw-loopback --capture-props='media.class=Audio/Sink node.name=hikaya-e2e-mic-in' \
  --playback-props='media.class=Audio/Source node.name=hikaya-e2e-mic' >/dev/null 2>&1 &
PIDS+=($!)
# Speaker: a sink nobody listens to; its monitor is the system-audio track.
pw-loopback --capture-props='media.class=Audio/Sink node.name=hikaya-e2e-speaker' \
  --playback-props='media.class=Audio/Source node.name=hikaya-e2e-speaker-out' >/dev/null 2>&1 &
PIDS+=($!)
for _ in $(seq 1 20); do
  [[ -n "$(node_id hikaya-e2e-mic)" && -n "$(node_id hikaya-e2e-speaker)" ]] && break
  sleep 0.25
done
wpctl set-default "$(node_id hikaya-e2e-mic)" || die "could not select the virtual microphone"
wpctl set-default "$(node_id hikaya-e2e-speaker)" || die "could not select the virtual speaker"

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
    "update_checks_enabled": False, "selected_microphone": None})
path.write_text(json.dumps(store))
PY

XDG_DATA_HOME="$WORK/xdg" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" --start-hidden --no-tray >"$WORK/app.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 120); do grep -q "transcribe-cpp initialized" "$WORK/app.log" && break; sleep 0.5; done
sleep 3

say "recording a meeting: the others speak first, then you, 30 s later"
app --toggle-meeting >/dev/null 2>&1
sleep 2
grep -q "Meeting .* started" "$WORK/app.log" || { tail -5 "$WORK/app.log"; die "the meeting did not start"; }
grep "Meeting .* started" "$WORK/app.log" | sed 's/^.*\] /   /'
timeout 30 pw-play --target hikaya-e2e-speaker "$CACHE/jfk.wav" || die "could not play to the speaker"
sleep 20   # into the second segment
timeout 30 pw-play --target hikaya-e2e-mic-in "$CACHE/jfk.wav" || die "could not play to the microphone"
sleep 2
app --toggle-meeting >/dev/null 2>&1

say "waiting for the transcript"
result=""
for _ in $(seq 1 180); do
  result="$(python3 - "$DATA/hikaya.db" <<'PY' 2>/dev/null || true
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
m = db.execute("SELECT id, status, duration_ms, system_audio FROM meetings ORDER BY id DESC LIMIT 1").fetchone()
if m and m[1] == "done":
    print(f"meeting {m[0]} {m[1]} {m[2]} ms, system audio {bool(m[3])}")
    for s, sp, t in db.execute("SELECT start_ms, speaker, text FROM meeting_lines WHERE meeting_id = ? ORDER BY start_ms, speaker", (m[0],)):
        print(f"{s} {sp} {t}")
PY
)"
  [[ -n "$result" ]] && break
  sleep 1
done
[[ -n "$result" ]] || { grep -iE "meeting|error" "$WORK/app.log" | tail -10; die "the meeting never finished"; }
echo "$result" | sed 's/^/   /'

lower="$(printf '%s' "$result" | tr '[:upper:]' '[:lower:]')"
[[ "$lower" == *"system audio true"* ]] || die "system audio was not captured"
echo "$lower" | grep -q "^0 others .*ask not what your country" || die "the others' speech is missing from the first segment"
echo "$lower" | grep -q "^30000 you .*ask not what your country" || die "your speech is missing from the second segment"
ls "$WORK/xdg/com.ahmedberrada.hikaya/meetings/"*/others-00000.wav "$WORK/xdg/com.ahmedberrada.hikaya/meetings/"*/you-00001.wav >/dev/null \
  || die "the audio segments were not saved"
say "PASS: both tracks were recorded, saved and transcribed under the right speaker"
