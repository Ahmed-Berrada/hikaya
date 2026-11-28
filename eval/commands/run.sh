#!/usr/bin/env bash
# Spoken-command check against real speech-to-text. See README.md.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$REPO/eval/commands"
CACHE="${HIKAYA_EVAL_CACHE:-$HOME/.cache/hikaya-eval}/commands"
BIN="$REPO/src-tauri/target/debug/handy"
[[ $# -ge 1 ]] || { echo "usage: $0 model.gguf [model.gguf…]" >&2; exit 2; }
[[ -x "$BIN" ]] || { echo "build the debug app first: bun run tauri dev" >&2; exit 2; }
LIB_DIR="$(dirname "$(find "$REPO/src-tauri/target/debug/build" -name 'libtranscribe.so.*' -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)")"
mkdir -p "$CACHE/voices" "$CACHE/wav"

if [[ ! -x "$CACHE/venv/bin/python" ]]; then
  uv venv -q --python 3.13 "$CACHE/venv"
  uv pip install -q --python "$CACHE/venv/bin/python" piper-tts soundfile soxr
fi
for v in en/en_US/lessac/medium/en_US-lessac-medium fr/fr_FR/siwis/medium/fr_FR-siwis-medium; do
  for ext in onnx onnx.json; do
    f="$CACHE/voices/$(basename "$v").$ext"
    [[ -s "$f" ]] || curl -fsSL -o "$f" "https://huggingface.co/rhasspy/piper-voices/resolve/main/$v.$ext"
  done
done

# Synthesize (16 kHz mono, as the app expects), once per sentence.
"$CACHE/venv/bin/python" -I - "$HERE/sentences.tsv" "$CACHE" <<'PY'
import sys, pathlib, hashlib, subprocess, soundfile as sf, soxr
tsv, cache = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
voices = {"en": "en_US-lessac-medium", "fr": "fr_FR-siwis-medium"}
for line in tsv.read_text().splitlines():
    if not line or line.startswith("#"):
        continue
    lang, _, text = line.split("\t")
    out = cache / "wav" / (hashlib.sha1(text.encode()).hexdigest()[:12] + ".wav")
    if out.exists():
        continue
    subprocess.run([sys.executable, "-m", "piper", "-m", str(cache / "voices" / f"{voices[lang]}.onnx"),
                    "-f", str(out), "--sentence-silence", "0.4"], input=text.encode(), check=True,
                   stderr=subprocess.DEVNULL)
    audio, rate = sf.read(out, dtype="float32")
    sf.write(out, soxr.resample(audio, rate, 16000), 16000, subtype="PCM_16")
PY

DATA="$(mktemp -d)"
trap 'rm -rf "$DATA"' EXIT
mkdir -p "$DATA/com.ahmedberrada.hikaya/models"
fails=0
for model in "$@"; do
  ln -sf "$(realpath "$model")" "$DATA/com.ahmedberrada.hikaya/models/"
  id="$(XDG_DATA_HOME="$DATA" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" --list-models 2>/dev/null \
    | sed -n "s/^ *✓ *\([^ ]*$(basename "$model")\) .*/\1/p" | head -1)"
  [[ -n "$id" ]] || { echo "the app does not know $(basename "$model")" >&2; exit 2; }
  echo "== $(basename "$model")"
  while IFS=$'\t' read -r lang expected text; do
    [[ -z "$lang" || "$lang" == \#* ]] && continue
    wav="$CACHE/wav/$(printf '%s' "$text" | sha1sum | cut -c1-12).wav"
    heard="$(XDG_DATA_HOME="$DATA" LD_LIBRARY_PATH="$LIB_DIR" "$BIN" --transcribe-file "$wav" --model "$id" \
      ${HIKAYA_EVAL_DEVICE:+--device-index "$HIKAYA_EVAL_DEVICE"} 2>/dev/null | sed -n 's/^text: //p' | tail -1)"
    got="$(cd "$REPO/crates/hikaya-core" && printf '%s' "$heard" | cargo run -q --example apply_commands)"
    norm() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed 's/[.]$//'; }
    if [[ "$(norm "$got")" == "$(norm "$(printf '%b' "$expected")")" ]]; then
      echo "   ok    $text"
    else
      fails=$((fails + 1))
      echo "   MISS  $text"
      echo "         heard: $heard"
    fi
  done < "$HERE/sentences.tsv"
done
echo "== $fails miss(es)"
