# 0001: Device topology and fork base

- Status: accepted
- Date: 2026-11-11

## Context

Dictation has to run where the user types and talks, and it must feel instant. The primary machine
is a Windows laptop with an NVIDIA RTX GPU. An always-on but weak home server (CPU only) and a phone
are also available. Data must stay on hardware the user owns.

## Decision

- **Real-time STT runs on the client GPU.** The hub is never in the dictation latency path.
- **The home server is an optional hub**: storage, sync, background jobs (re-transcription,
  diarization, summaries) and the phone backend. It is reachable only over a private network.
- **The phone is a capture device first** (PWA). On-device mobile STT is deferred.
- **Fork base: [Handy](https://github.com/cjpais/Handy)** (MIT). It already provides a Tauri 2 app,
  GPU-backed whisper.cpp/Parakeet inference, global shortcuts, text injection on Windows/macOS/Linux,
  history and LLM post-processing. Meeting capture ideas (system loopback, VAD chunking, summaries)
  are adapted from [Meetily](https://github.com/Zackriya-Solutions/meeting-minutes) (MIT).
- The fork keeps an `upstream` remote and merges upstream regularly. Hikaya features live in clearly
  separated modules to keep merge conflicts small.

## Consequences

- Windows installers are built on Windows (a CI runner or the developer's machine), not cross-compiled.
- A sync protocol and device authentication are needed before the hub ships (v0.4).
- Rebranding touches bundle identifiers. It is done once, in its own PR, before the first release.
