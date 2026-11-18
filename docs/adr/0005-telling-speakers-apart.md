# 0005: Telling speakers apart within a track

- Status: proposed
- Date: 2026-11-18

## Context

A meeting's lines say **You** (this computer's microphone), **Others** (what the computer plays)
or **Room** (a phone recording, ADR 0004). On a call with three people, every remote voice is
"Others"; in a café recorded on the phone, everyone is "Room". Summaries then say "Others: send
the deck" instead of naming who. The roadmap lists "speakers beyond You/Others" as the open item
of meetings.

Splitting a track by voice ("speaker diarization") needs two models: one that finds where speech
starts and stops and where the voice changes (pyannote segmentation), and one that turns a few
seconds of a voice into a vector (a speaker embedding such as WeSpeaker or 3D-Speaker). Close
vectors are clustered into speakers. All run offline on a CPU, faster than real time.

Options found (November 2026):

| Option                                 | What it is                                                         | Cost to Hikaya                                                |
| -------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------- |
| `sherpa-onnx` crate (k2-fsa, official) | Rust bindings to sherpa-onnx's C API, offline diarization included | prebuilt native libraries (ONNX Runtime) on Windows and Linux |
| `pyannote-rs`                          | Rust: segmentation-3.0 + WeSpeaker on ONNX Runtime                 | ONNX Runtime; simpler pipeline, less maintained               |
| `speakrs`                              | Rust port of the pyannote community-1 pipeline                     | ONNX Runtime; young project                                   |
| Python pyannote                        | the reference pipeline                                             | a Python runtime in the app: rejected                         |

None runs on transcribe-cpp/GGML, which the app already ships, so each adds ONNX Runtime
(roughly 10–20 MB) to the installer.

## Decision (proposed)

- Diarize **after** a meeting, not live: once it is done, each "Others" and "Room" track is split
  into speakers, and each transcript line takes the speaker who talks most during it. "You" is
  never split. Live transcription stays as it is.
- Use the official **`sherpa-onnx`** crate, with pyannote segmentation-3.0 and a 3D-Speaker or
  WeSpeaker embedding model, downloaded on first use like speech models (about 30 MB together),
  never bundled. Clustering uses a distance threshold, so the number of speakers is found, not
  asked.
- Speakers are stored as **Speaker 1, Speaker 2…** per meeting, and the user can rename them
  ("Speaker 2" → "Sarah") in the meeting view; the names go into the summary prompt and the
  exports, and sync to the hub as plain text.
- Off by default until measured: a setting "Tell speakers apart (beta)".

## Before accepting

1. A spike on Linux with an openly licensed multi-speaker set (the AMI meeting corpus, CC BY 4.0):
   diarization error rate and speed on the ProBook CPU, in English and French (VoxPopuli or
   FLEURS mixes for French).
2. The Windows build from CI: installer size before and after, and a meeting on the owner's laptop.
3. Accept only if a 30-minute meeting takes under two minutes on the laptop and speakers are
   right on most lines; otherwise keep You/Others/Room.

## Measurements (2026-11-18)

`eval/diarization/run.py`: sherpa-onnx 1.13, pyannote segmentation-3.0, two AMI test meetings
(4 speakers each, headset mix), ProBook CPU (i7-10510U, 4 threads). DER with a 0.25 s collar,
overlap skipped.

| Meeting | Embedding          | Clustering           | DER    | Speed         |
| ------- | ------------------ | -------------------- | ------ | ------------- |
| ES2004a | 3D-Speaker CAM++   | any threshold        | ≈ 56%  | 10× real time |
| ES2004a | WeSpeaker ResNet34 | threshold 1.0        | 12.7%  | 6× real time  |
| ES2004a | WeSpeaker ResNet34 | 4 speakers given     | 21.4%  | 6× real time  |
| IS1009a | WeSpeaker ResNet34 | threshold 0.6 to 1.1 | 38–46% | 6× real time  |
| IS1009a | WeSpeaker ResNet34 | 4 speakers given     | 39.7%  | 6× real time  |

Speed is fine: a 30-minute meeting takes about five minutes on this CPU, less on the laptop. The
accuracy is not: no single threshold works for both meetings, and even the true speaker count
leaves IS1009a at 40%. The default CAM++ model does not separate the voices at all. **Not
accepted** with this pipeline.

Next candidate: the pyannote **community-1** pipeline, whose clustering (VBx on PLDA-scored
embeddings) is what makes it robust on AMI; it runs in Rust through `speakrs` on ONNX Runtime.
The same script and meetings decide, with the same bar: under 20% DER on both.

## Consequences

- Summaries and action items can name people, and exports become readable for calls with several
  people.
- A second native runtime in the app (ONNX Runtime beside GGML), with its own updates and
  advisories to follow.
- Echo removal (ADR 0003) still works on tracks, before diarization, so it is unaffected.
