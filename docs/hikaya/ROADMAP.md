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

## Working method
Ship a small version, use it daily, then fix what hurts. Each phase ends with a tagged release.

## Device roles (see [ADR 0001](../adr/0001-device-topology.md))
| Device | Role |
|---|---|
| Desktop/laptop with a GPU | Main client. Dictation and live transcription run on the local GPU. |
| Self-hosted hub (optional) | Always-on home server: storage, sync between devices, background jobs (re-transcription, diarization, summaries). |
| Phone | Capture only: a web app over a private network for in-person meetings and voice notes. |

## Phases
### v0.1: Dictation baseline
- Upstream Handy, rebranded, with EN/FR defaults and Ollama post-processing presets.
- Windows build from CI. Daily use, with issues filed for anything that gets in the way.

### v0.2: Learning loop
- Personal dictionary that biases the STT prompt and the LLM cleanup.
- Correction mining: edit a past dictation, a word-level diff is stored, and a substitution seen
  2 or more times is proposed as a replacement rule.
- Recent corrections become few-shot examples for cleanup. Stats page (WPM, top corrections).

### v0.3: Meetings
- Mic and system loopback recorded as separate tracks (you vs. others), written to disk continuously
  so a crash doesn't lose the recording.
- Live chunked transcription, then a final higher-quality pass with diarization.
- **Raw transcript** with timestamps and speakers, plus a **summary** (map-reduce via Ollama).
- Export to Markdown, TXT and SRT. Audio retention settings.

### v0.4: Hub and sync
- Rust (axum) service in Docker, reachable only over a private network (e.g. Tailscale), with
  per-device tokens.
- Sync for dictations, meetings, dictionary and rules. Job queue for background work.

### v0.5: Phone capture
- PWA recorder with chunked upload to the hub, and transcript/summary viewer.
- iOS Shortcut: record, send to the hub, copy the result to the clipboard.

## Evaluation data
Accuracy is measured with word error rate (WER) on openly licensed corpora (Mozilla Common Voice,
FLEURS, VoxPopuli; EN and FR) plus the user's own recordings, which are never committed. We don't
train on scraped video platforms; their terms forbid it. Personalization comes from the learning
loop. Fine-tuning is only considered if the benchmarks show a gap.
