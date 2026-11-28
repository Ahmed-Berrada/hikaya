# Command mode

Each case in `cases.json` is a selected text, a spoken instruction, phrases the result must keep
and phrases it must not contain (`max_ratio` caps the length for "make it shorter"). `run.py` sends
the app's own prompt (`crates/hikaya-core/src/prompts/command.txt`) to an OpenAI-compatible
endpoint, Ollama by default, and cleans the reply as the app does.

```bash
python3 eval/command/run.py                      # qwen3:4b-instruct on 127.0.0.1:11434
python3 eval/command/run.py --model qwen3:8b     # a bigger model, such as on the laptop GPU
```

On 2026-11-28 `qwen3:4b-instruct` passed 5 of 7: it shortens, translates both ways, makes lists
and leaves a question unanswered, but a formal rewrite kept "salut" and a grammar fix kept "euh".
Telling it to drop those made it translate French text into English, which is worse, so the
prompt does not ask. The app also refuses a result whose language changed when the instruction did
not ask for a translation (`hikaya_core::command::switched_language`).
