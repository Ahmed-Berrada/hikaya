# Cleanup prompt regression test

Checks that AI cleanup of a dictation never translates mixed English/French text, answers a
question, or obeys an instruction inside the transcript (issue #6).

```bash
python3 eval/cleanup/run.py                          # Hikaya's prompt, local Ollama, qwen3:4b-instruct
python3 eval/cleanup/run.py --model <model> --base-url http://127.0.0.1:11434/v1
python3 eval/cleanup/run.py --prompt some-other-prompt.txt
```

It needs only Python 3 and an OpenAI-compatible endpoint. Each case in `cases.json` lists phrases
that must survive (`keep`) and phrases that must not appear (`drop`). The script sends the raw
transcript, as the app does for a custom provider. It does **not** run Hikaya's own steps around
the model:

- before the model, fillers are already removed in code (`hikaya-core::fillers`);
- after the model, a cleanup that brought in too many new words is rejected and the text is
  pasted as spoken (`hikaya-core::faithful`).

So a `FAIL` here means "the model got it wrong", not "the user got wrong text".

## Results so far

Measured on 2026-11-16 with `qwen3:4b-instruct` (Q4, CPU, Ollama 0.40.16), temperature 0.

| Prompt                                             | Passed |
| -------------------------------------------------- | ------ |
| Hikaya (`src-tauri/src/hikaya/cleanup_prompt.txt`) | 7 / 12 |
| Handy's default prompt                             | 6 / 12 |
| A shorter "copy the transcript" variant            | 6 / 12 |

A 4B model translates one half of a mixed sentence about half the time, whatever the wording.
Every translated or rewritten output from these runs is in the unit tests of
`crates/hikaya-core/src/faithful.rs`, and the check rejects all of them. Larger models on a GPU
have not been measured yet.
