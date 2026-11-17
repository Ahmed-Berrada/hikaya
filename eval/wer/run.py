#!/usr/bin/env python3
"""Word error rate (WER) of speech models, through the real app.

Downloads the first clips of the FLEURS test set (Google, CC BY 4.0: read
Wikipedia sentences with exact transcripts) in French and English,
transcribes every clip with the app's headless mode (`--transcribe-file`)
for each model, and prints WER and speed per model and language.

MINDS-14 was tried first and dropped: its references often cover only the
start of what the caller said, so a correct model looked wrong.

    python3 eval/wer/run.py --models whisper-small whisper-large-v3-turbo
    python3 eval/wer/run.py --models parakeet-tdt-0.6b-v3 --samples 50 --language fr

Models are catalog ids; their Q8_0 file is downloaded to the cache once. The
app runs with a throwaway data directory. Build the debug app first
(`bun run tauri dev`, then quit), or pass --bin.

Standard library only. Audio and transcripts stay in the cache, never in the
repository.
"""
import argparse
import array
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unicodedata
import urllib.parse
import urllib.request
import wave

ROOT = pathlib.Path(__file__).resolve().parents[2]
CATALOG = ROOT / "src-tauri/src/catalog/catalog.json"
CACHE = pathlib.Path(os.environ.get("HIKAYA_EVAL_CACHE", pathlib.Path.home() / ".cache/hikaya-eval"))
FLEURS = "https://huggingface.co/datasets/google/fleurs/resolve/main/data/{config}/{file}"
DATASET = "google/fleurs (test)"
CONFIGS = {"fr": "fr_fr", "en": "en_us"}
USER_AGENT = "hikaya-eval/1.0"


def fetch(url, dest=None, timeout=120):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        if dest is None:
            return response.read()
        tmp = dest.with_suffix(dest.suffix + ".part")
        with open(tmp, "wb") as out:
            shutil.copyfileobj(response, out)
        tmp.rename(dest)
        return None


def _ulaw(byte):
    byte = ~byte & 0xFF
    sign, exponent, mantissa = byte & 0x80, (byte >> 4) & 0x07, byte & 0x0F
    value = ((mantissa << 3) + 0x84) << exponent
    value -= 0x84
    return -value if sign else value


def _alaw(byte):
    byte ^= 0x55
    sign, exponent, mantissa = byte & 0x80, (byte >> 4) & 0x07, byte & 0x0F
    value = (mantissa << 4) + 8 if exponent == 0 else ((mantissa << 4) + 0x108) << (exponent - 1)
    return value if sign else -value


def read_wav(path):
    """(interleaved samples scaled to the 16-bit range, channels, rate). Reads
    16-bit PCM, 32-bit float (FLEURS), and mu-law and A-law (telephone audio)."""
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ValueError(f"{path}: not a WAV file")
    pos, fmt, payload = 12, None, None
    while pos + 8 <= len(data):
        chunk, size = data[pos:pos + 4], int.from_bytes(data[pos + 4:pos + 8], "little")
        body = data[pos + 8:pos + 8 + size]
        if chunk == b"fmt ":
            fmt = (int.from_bytes(body[0:2], "little"), int.from_bytes(body[2:4], "little"),
                   int.from_bytes(body[4:8], "little"), int.from_bytes(body[14:16], "little"))
        elif chunk == b"data":
            payload = body
        pos += 8 + size + (size & 1)
    if fmt is None or payload is None:
        raise ValueError(f"{path}: missing fmt or data chunk")
    tag, channels, rate, bits = fmt
    if tag == 1 and bits == 16:
        values = array.array("h", payload[: len(payload) // 2 * 2])
        if sys.byteorder == "big":
            values.byteswap()
        return list(values), channels, rate
    if tag == 3 and bits == 32:
        values = array.array("f", payload[: len(payload) // 4 * 4])
        if sys.byteorder == "big":
            values.byteswap()
        return [v * 32767 for v in values], channels, rate
    if tag == 7:
        return [_ulaw(b) for b in payload], channels, rate
    if tag == 6:
        return [_alaw(b) for b in payload], channels, rate
    raise ValueError(f"{path}: WAV format {tag} with {bits} bits is not supported")


def to_16k_mono(source, dest):
    """Writes `source` as 16 kHz mono 16-bit, by linear interpolation."""
    samples, channels, rate = read_wav(source)
    mono = [sum(samples[i:i + channels]) / channels for i in range(0, len(samples), channels)]
    ratio = rate / 16000
    out = array.array("h")
    for i in range(int(len(mono) / ratio)):
        pos = i * ratio
        j = int(pos)
        a = mono[j]
        b = mono[j + 1] if j + 1 < len(mono) else a
        out.append(int(max(-32768, min(32767, a + (b - a) * (pos - j)))))
    if sys.byteorder == "big":
        out.byteswap()
    with wave.open(str(dest), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(out.tobytes())


def samples(language, count):
    """The first `count` test clips of a language: [(wav path, reference text)].
    Streams the archive and stops once it has enough, so most of it is never
    downloaded."""
    import tarfile

    folder = CACHE / "fleurs" / language
    folder.mkdir(parents=True, exist_ok=True)
    index = folder / "index.json"
    items = json.loads(index.read_text()) if index.exists() else []
    if len(items) < count:
        tsv = fetch(FLEURS.format(config=CONFIGS[language], file="test.tsv")).decode("utf-8")
        # Columns: id, file name, raw transcription, normalized transcription, ...
        references = {}
        for line in tsv.splitlines():
            columns = line.split("\t")
            if len(columns) >= 3:
                references[columns[1]] = columns[2]
        have = {item["wav"] for item in items}
        url = FLEURS.format(config=CONFIGS[language], file="audio/test.tar.gz")
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(request, timeout=600) as response, tarfile.open(fileobj=response, mode="r|gz") as tar:
            for member in tar:
                name = pathlib.Path(member.name).name
                if not member.isfile() or name not in references or name in have:
                    continue
                raw = folder / f"{name}.orig"
                with tar.extractfile(member) as source, open(raw, "wb") as out:
                    shutil.copyfileobj(source, out)
                to_16k_mono(raw, folder / name)
                raw.unlink()
                items.append({"wav": name, "text": references[name]})
                if len(items) >= count:
                    break
        index.write_text(json.dumps(items, ensure_ascii=False, indent=1))
    return [(folder / item["wav"], item["text"]) for item in items[:count]]


def normalize(text):
    """Lowercase words, accents kept, punctuation and spacing ignored."""
    text = unicodedata.normalize("NFKC", text).lower().replace("’", "'")
    text = re.sub(r"[^\w']+", " ", text)
    return text.replace("'", "' ").split()


def errors(reference, hypothesis):
    """Word-level edit distance."""
    ref, hyp = normalize(reference), normalize(hypothesis)
    previous = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        current = [i]
        for j, h in enumerate(hyp, 1):
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (r != h)))
        previous = current
    return previous[-1], len(ref)


def model_file(model_id):
    """Downloads a catalog model's Q8_0 (or default) file once; returns (path, catalog id)."""
    catalog = json.loads(CATALOG.read_text())["models"]
    entry = next((m for m in catalog if m["id"] == model_id), None)
    if entry is None:
        sys.exit(f"unknown model {model_id!r}; see {CATALOG}")
    quant = "Q8_0" if "Q8_0" in entry["files"] else entry.get("quant")
    name = entry["files"][quant]["filename"]
    path = CACHE / "models" / name
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        url = f"https://huggingface.co/{entry['repository']}/resolve/{entry['revision']}/{name}"
        print(f"downloading {name} ({entry['files'][quant]['size'] / 1e6:.0f} MB)", flush=True)
        fetch(url, path, timeout=3600)
    return path, f"{entry['repository']}/{name}"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--models", nargs="+", default=["whisper-small"])
    parser.add_argument("--language", choices=["fr", "en", "both"], default="both")
    parser.add_argument("--samples", type=int, default=40)
    parser.add_argument(
        "--spoken",
        choices=["auto", "given"],
        default="auto",
        help="auto: the app detects the language, as for a user who speaks both; given: it is told",
    )
    parser.add_argument(
        "--device-index",
        type=int,
        help="compute device for the app (see `--list-devices`); default: the app's own choice",
    )
    parser.add_argument("--bin", type=pathlib.Path, default=ROOT / "src-tauri/target/debug/handy")
    parser.add_argument("--json", type=pathlib.Path, help="also write the results here")
    args = parser.parse_args()

    if not args.bin.exists():
        sys.exit(f"{args.bin} not found; build the app first")
    env = dict(os.environ)
    if sys.platform.startswith("linux"):
        libs = sorted((ROOT / "src-tauri/target").glob("*/build/transcribe-cpp-sys-*/out/lib/libtranscribe.so*"),
                      key=lambda p: p.stat().st_mtime)
        if libs:
            env["LD_LIBRARY_PATH"] = str(libs[-1].parent)

    languages = ["fr", "en"] if args.language == "both" else [args.language]
    clips = {lang: samples(lang, args.samples) for lang in languages}
    results = []
    with tempfile.TemporaryDirectory(prefix="hikaya-wer-") as tmp:
        data = pathlib.Path(tmp) / "xdg/com.ahmedberrada.hikaya"
        (data / "models").mkdir(parents=True)
        env["XDG_DATA_HOME"] = str(pathlib.Path(tmp) / "xdg")
        env["APPDATA"] = str(pathlib.Path(tmp) / "xdg")
        for model_id in args.models:
            path, catalog_id = model_file(model_id)
            link = data / "models" / path.name
            if not link.exists():
                # Windows needs special rights for symlinks: fall back to a
                # hard link (same drive), then to a copy.
                try:
                    link.symlink_to(path)
                except OSError:
                    try:
                        os.link(path, link)
                    except OSError:
                        shutil.copy(path, link)
            for lang in languages:
                settings = data / "settings_store.json"
                store = json.loads(settings.read_text()) if settings.exists() else {"settings": {}}
                store["settings"].update({"selected_language": "auto" if args.spoken == "auto" else lang,
                                          "spoken_languages": ["en", "fr"]})
                settings.write_text(json.dumps(store))
                total_errors = total_words = 0
                audio_secs = compute_ms = 0.0
                failures = 0
                for wav, reference in clips[lang]:
                    run = subprocess.run(
                        [str(args.bin), "-f", str(wav), "--model", catalog_id, "--json"]
                        + (["--device-index", str(args.device_index)] if args.device_index is not None else []),
                        env=env, capture_output=True, text=True, timeout=600,
                    )
                    line = next((l for l in run.stdout.splitlines() if l.startswith("{")), None)
                    if run.returncode != 0 or line is None:
                        failures += 1
                        continue
                    out = json.loads(line)
                    e, n = errors(reference, out["text"])
                    total_errors += e
                    total_words += n
                    audio_secs += out["audio_secs"]
                    compute_ms += out["best_ms"]
                wer = total_errors / total_words if total_words else float("nan")
                rtf = audio_secs * 1000 / compute_ms if compute_ms else 0
                results.append({"model": model_id, "language": lang, "clips": len(clips[lang]) - failures,
                                "failures": failures, "wer": round(wer, 4), "speed_x_realtime": round(rtf, 1)})
                print(f"{model_id:<28} {lang}  WER {wer:6.1%}  {rtf:5.1f}x real time  "
                      f"({len(clips[lang]) - failures} clips{f', {failures} failed' if failures else ''})", flush=True)

    print("\n| Model | Language | WER | Speed (x real time) | Clips |\n|---|---|---|---|---|")
    for r in results:
        print(f"| {r['model']} | {r['language']} | {r['wer']:.1%} | {r['speed_x_realtime']} | {r['clips']} |")
    if args.json:
        args.json.write_text(json.dumps({"dataset": DATASET, "spoken": args.spoken, "results": results}, indent=2) + "\n")


if __name__ == "__main__":
    main()
