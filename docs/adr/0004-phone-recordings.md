# 0004: Recording on the phone, transcribing on the computer

- Status: accepted
- Date: 2026-11-17

## Context

The owner wants to record in-person meetings and voice notes on an iPhone. The phone reaches the
hub through the web viewer ([ADR 0002](0002-linux-dev-and-hub-archive.md)). The hub runs on a slow
CPU and, by design, does not do heavy speech recognition; the GPU computer does.

## Decision

- **The phone records in the web viewer** with `MediaRecorder` (AAC in MP4 on iPhone Safari, Opus
  in WebM elsewhere) and uploads the audio in numbered chunks every few seconds while recording, so
  a dropped connection or a closed page loses at most the last chunk.
- **The hub keeps the recording encrypted** (chunks are rows in the SQLCipher database) and marks
  it _pending_ once the phone says it is finished.
- **The desktop app transcribes it.** On each sync it claims one pending recording, downloads it,
  decodes it with the same decoder as "Transcribe File", transcribes it with the same pipeline as
  meetings (30-second segments, speech check, filler and rule stage), summarizes it when a
  provider is set up, and sends the result back as a meeting. A claim that is not completed within
  two hours can be taken by another device.
- **Speaker:** a phone recording is one microphone in a room, so its lines are labelled **Room**,
  a third speaker next to You and Others.
- **After transcription the hub deletes the audio**; the meeting keeps the transcript and summary.
  A recording that fails to transcribe keeps its audio for a retry.

## Consequences

- iPhone Safari stops recording when the screen locks or the page goes to the background. The page
  keeps the screen awake (Wake Lock) and tells the user to leave it open. A native app would avoid
  this; it is not planned.
- Nothing is transcribed until a desktop app with Hikaya syncs. The phone shows "waiting for your
  computer".
- WebM recordings (Android, desktop browsers) need the desktop app's decoder to read Matroska.
