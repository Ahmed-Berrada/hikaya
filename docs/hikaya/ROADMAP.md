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

### v0.1: Dictation baseline

- Upstream Handy, rebranded, with EN/FR defaults and Ollama post-processing presets.
- Windows build from CI. Daily use, with issues filed for anything that gets in the way.

### v0.2: Learning loop

- Personal dictionary that biases the STT prompt and the LLM cleanup.
- Correction mining: edit a past dictation, a word-level diff is stored, and a substitution seen
  2 or more times is proposed as a replacement rule.
- Recent corrections become few-shot examples for cleanup. Stats page (WPM, top corrections).

### v0.3: Hub, vault and sync

- Rust (axum) service in Docker, reachable only over a private network (e.g. Tailscale), with
  per-device tokens.
- Encrypted store (SQLCipher and `age`). The hub stays locked after a restart until the passphrase
  is entered; clients queue their data meanwhile.
- Sync for dictations, dictionary and rules from a client-side outbox. Web viewer with search.
- Encrypted backups, with a tested restore.

### v0.4: Meetings

- Mic and system loopback recorded as separate tracks (you vs. others), written to disk continuously
  so a crash doesn't lose the recording.
- Live chunked transcription, then a final higher-quality pass with diarization.
- **Raw transcript** with timestamps and speakers, plus a **summary** (map-reduce via Ollama) made
  on the GPU client, with the hub as fallback.
- Export to Markdown, TXT and SRT. Audio retention settings. Recording indicator and consent reminder.

### v0.5: Phone capture

- PWA recorder with chunked upload to the hub, and transcript/summary viewer.
- iOS Shortcut: record, send to the hub, copy the result to the clipboard.

## Evaluation data

Accuracy is measured with word error rate (WER) on openly licensed corpora (Mozilla Common Voice,
FLEURS, VoxPopuli; EN and FR) plus the user's own recordings, which are never committed. We don't
train on scraped video platforms; their terms forbid it. Personalization comes from the learning
loop. Fine-tuning is only considered if the benchmarks show a gap.

If fine-tuning is needed, it runs on the owner's Windows PC with an NVIDIA GPU (a few hours per
run), using the user's corrected dictations plus openly licensed EN/FR corpora. The result is a
model file loaded like any other; nothing about training ships in the app.
