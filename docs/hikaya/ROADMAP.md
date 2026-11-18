# Hikaya roadmap

Hikaya (حكاية, Darija for "a story, a telling") is a local-first voice tool built on
[Handy](https://github.com/cjpais/Handy). It has three jobs:

1. **Dictate anywhere.** Press a hotkey, speak, and the text is typed into the focused app.
2. **Learn from you.** Your corrections, names and jargon improve later transcriptions.
3. **Keep meeting notes.** Record your mic and system audio, then store a **raw transcript**
   and a **summary** (TL;DR, decisions, action items).

Everything (audio, transcripts, learned data) stays on hardware you own. Nothing goes to a cloud service.

Languages: English, French, and mixed EN/FR (code-switching). LLM work (cleanup and summaries) runs
locally through [Ollama](https://ollama.com).

## Contributing

Development and automated tests run on Linux; Windows installers are built by CI for each release
(see [ADR 0002](../adr/0002-linux-dev-and-hub-archive.md)). To build on Windows yourself, see
[DEV_SETUP_WINDOWS.md](DEV_SETUP_WINDOWS.md).

## Platforms

| Platform    | Status                                                              |
| ----------- | ------------------------------------------------------------------- |
| Windows x64 | Main target. Every release is checked by hand on it.                |
| Linux x64   | Built and tested automatically; used for development.               |
| iPhone      | Nice to have: a web app served by the hub (v0.5), not a native app. |
| macOS       | Not supported. Builds were removed.                                 |
| Windows ARM | Not supported for now (#15).                                        |

## Working method

Ship a small version, use it daily, then fix what hurts. Each phase ends with a tagged release.

## Device roles (see ADRs [0001](../adr/0001-device-topology.md) and [0002](../adr/0002-linux-dev-and-hub-archive.md))

| Device                    | Role                                                                                                                 |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Desktop/laptop with a GPU | Main client. Dictation, live transcription and summaries run on the local GPU. Keeps a local copy and works offline. |
| Self-hosted hub           | Always-on home server: the complete, encrypted archive, sync between devices, and background jobs as a fallback.     |
| Phone                     | Capture only: a web app over a private network for in-person meetings and voice notes.                               |

## Phases

Status as of 2026-11-18: v0.1–v0.4 and phone recording (v0.5) are built and tested on Linux; 0.4.0
is the build waiting for the first Windows test. The hub runs on the owner's home server. Model accuracy is measured in
[BASELINE.md](BASELINE.md).

### v0.1: Dictation baseline (done)

- Upstream Handy, rebranded, with EN/FR defaults and Ollama post-processing presets.
- Windows build from CI. Daily use, with issues filed for anything that gets in the way.

### v0.2: Learning loop (done)

- Personal dictionary that biases the STT prompt and the LLM cleanup.
- Correction mining: edit a past dictation, a word-level diff is stored, and a substitution seen
  2 or more times is proposed as a replacement rule.
- Recent corrections become few-shot examples for cleanup. Stats page (WPM, top corrections).

Done: correction mining, rule proposals, replacement rules, EN+FR filler removal, the mixed-language
cleanup prompt with a guard against translation, the user's vocabulary (custom words and rule
replacements) in that prompt, and the stats panel. Left: aliases for the custom-words dictionary.

### v0.3: Hub, vault and sync (mostly done, in 0.2.0)

- Rust (axum) service in Docker, reachable only over a private network (e.g. Tailscale), with
  per-device tokens.
- Encrypted store (SQLCipher and `age`). The hub stays locked after a restart until the passphrase
  is entered; clients queue their data meanwhile.
- Sync for dictations, dictionary and rules from a client-side outbox. Web viewer with search.
- Encrypted backups, with a tested restore.

Done: vault and locked state, device tokens, sync of dictations, rules and corrections, search, web
viewer, encrypted snapshots in the nightly backup, Docker deployment on the home server. Left:
syncing the custom-words dictionary, and `age` encryption for audio (no audio is synced yet; it
comes with meetings).

### v0.4: Meetings (done, pending the Windows check)

- Mic and system loopback recorded as separate tracks (you vs. others), written to disk continuously
  so a crash doesn't lose the recording.
- Live chunked transcription, then a final higher-quality pass with diarization.
- **Raw transcript** with timestamps and speakers, plus a **summary** (map-reduce via Ollama) made
  on the GPU client, with the hub as fallback.
- Export to Markdown, TXT and SRT. Audio retention settings. Recording indicator and consent reminder.

Done: two tracks (WASAPI loopback on Windows, PipeWire monitor on Linux), crash-safe segments,
transcription while recording, echo removal, summaries with the configured provider, Markdown and
SRT export, consent reminder (ADR 0003), sync to the hub as text, audio retention, and a tray line
with Stop. Left: speakers beyond You/Others, and the Windows hand check.

### v0.5: Phone capture (recording done)

- Done: the hub's web viewer works on the iPhone (search, unlock, add to home screen).
- Done: recording on the phone (ADR 0004). The viewer's Record tab uploads chunks while it
  records; the hub keeps them encrypted; the desktop app transcribes them on its next sync as
  **Room** lines, summarizes, and the hub then deletes the audio. Tested end to end on Linux
  (`scripts/hikaya/e2e-phone.sh`); still to check on a real iPhone.
- iOS Shortcut: record, send to the hub, copy the result to the clipboard.

## Evaluation data

Accuracy is measured with word error rate (WER) on openly licensed corpora (Mozilla Common Voice,
FLEURS, VoxPopuli; EN and FR) plus the user's own recordings, which are never committed. We don't
train on scraped video platforms; their terms forbid it. Personalization comes from the learning
loop. Fine-tuning is only considered if the benchmarks show a gap.

If fine-tuning is needed, it runs on the owner's Windows PC with an NVIDIA GPU (a few hours per
run), using the user's corrected dictations plus openly licensed EN/FR corpora. The result is a
model file loaded like any other; nothing about training ships in the app.
