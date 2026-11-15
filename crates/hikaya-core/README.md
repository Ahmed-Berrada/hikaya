# hikaya-core

Platform-independent logic for [Hikaya](../../README.md). No Tauri, no audio, no I/O: everything
here runs and is tested on any machine.

| Module        | What it does                                                                       |
| ------------- | ---------------------------------------------------------------------------------- |
| `token`       | Splits text into words with their surrounding punctuation, and joins them back.    |
| `fillers`     | Removes hesitation words for all of the speaker's languages at once (EN/FR mixed). |
| `rules`       | Whole-word replacement rules ("get hub" → "GitHub"), optionally per application.   |
| `corrections` | Turns a user's edit into word substitutions and proposes rules from repeated ones. |

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
