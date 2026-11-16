# 0003: Meeting capture and transcripts

- Status: accepted
- Date: 2026-11-16

## Context

Roadmap v0.4: record a meeting (in person or a call) and keep a **raw transcript** and a
**summary**. The main machine is Windows; development and automated tests are on Linux
([ADR 0002](0002-linux-dev-and-hub-archive.md)). Recordings can hold other people's voices, so
they need the same care as the dictation archive, and in France recording someone requires their
consent.

## Decision

**Two tracks.** The microphone ("You") and the system output ("Others": the remote people on a
call) are recorded separately, both at 16 kHz mono. Two tracks give a first speaker split for
free, with no diarization model.

- Microphone: Handy's `handy-recorder`, as for dictation.
- System audio on Linux: the PipeWire/PulseAudio monitor of the default output, which
  `handy-recorder` already lists (`is_monitor`).
- System audio on Windows: WASAPI loopback, by opening the default output device as an input with
  `cpal` (already in the dependency tree). This path can only be compiled on Linux; it is checked by
  hand on Windows through the release checklist.
- If the system track cannot be opened, the meeting records the microphone alone and says so.

**Crash-safe files.** Each track is written as consecutive 30-second WAV segments in
`<app data>/meetings/<id>/`. A crash loses at most the segment being written. Segments are also the
unit of transcription.

**Transcription while recording.** Each closed segment goes through the same transcription manager
and model as dictation, then Hikaya's filler and rule stage. The transcript is a list of
`(start, end, speaker, text)` lines, saved as they arrive, so the raw transcript exists even if the
app stops mid-meeting. Silent segments are skipped with the existing VAD.

**Echo.** With loudspeakers, the microphone also hears the other people, which would duplicate
their words under "You". The first version recommends headphones and drops "You" lines that
repeat a nearly identical "Others" line from the same moment. Real echo cancellation and
diarization beyond two speakers are later work.

**Summary.** After the meeting, the transcript is summarized through the post-processing provider
the user configured (Ollama by default) with a Hikaya prompt that returns a TL;DR, key points,
decisions, action items and open questions, in the meeting's language. Long transcripts are
summarized in parts, then the parts together. The faithfulness guard does not apply: a summary is
supposed to use new words.

**Storage and sync.** Meetings and transcript lines live in `hikaya.db`; audio stays on the device.
Transcripts and summaries sync to the hub as text, encrypted at rest there like dictations. Audio
sync, and `age` encryption for it, comes later.

**Consent.** While recording, the tray icon and the app show that a meeting is being recorded.
The first time, the app reminds the user that the other people must agree to be recorded.

## Consequences

- Windows system capture ships untested by automation. The release checklist gains a meeting test.
- Audio files on the device are not encrypted (the same as Handy's dictation recordings). A
  retention setting removes them after a chosen number of days.
- Speaker labels are only "You" and "Others" in the first version.
- Long meetings make many model calls; on CPU-only machines transcripts can lag behind the meeting.
