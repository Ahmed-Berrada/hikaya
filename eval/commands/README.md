# Spoken commands

Checks that the voice commands ("new line", "à la ligne", "delete that", "supprime ça"…) survive
real speech-to-text: a command only acts when the model writes it apart from the sentence, with a
pause mark around it, and spelled as `crates/hikaya-core/src/commands.rs` expects.

`run.sh` synthesizes each sentence of `sentences.tsv` with [Piper](https://github.com/OHF-Voice/piper1-gpl),
transcribes it with the debug app (`--transcribe-file`), applies the commands, and compares the
result with the expected text. Case and the final period are ignored; line breaks are not.

```bash
bun run tauri dev   # once, then quit: builds the debug app
eval/commands/run.sh ~/.cache/hikaya-eval/models/whisper-small-Q8_0.gguf [more models…]
```

It needs `uv` and network access the first time. Everything it downloads or makes (a Python
environment, two voices, the audio) goes into `~/.cache/hikaya-eval/commands/`, never into the repo.
The voices: `fr_FR-siwis-medium` (SIWIS, CC BY 4.0) and `en_US-lessac-medium` (Blizzard 2013
Lessac data, licensed for research use only, which is why it is downloaded here and not shipped).

A synthetic voice is harder to hear than a person: a miss here is a warning, not proof. On
2026-11-28 "scratch that" was heard as "crutch that" and "efface ça" as "et face à ça" by Parakeet
and Whisper Small, which is why the app shows "delete that" and "supprime ça".
