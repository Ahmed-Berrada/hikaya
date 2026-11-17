# Speech model accuracy (WER)

Measures the word error rate of speech models in French and English, through the real app's
headless mode, so the numbers include Hikaya's audio loading and the model settings it uses.

```bash
python3 eval/wer/run.py --models whisper-small whisper-large-v3-turbo parakeet-tdt-0.6b-v3
python3 eval/wer/run.py --models whisper-large-v3-turbo --language fr --samples 100
python3 eval/wer/run.py --spoken given    # tell the model the language instead of detecting it
```

- **Data:** the first clips of the [FLEURS](https://huggingface.co/datasets/google/fleurs) test
  set (Google, CC BY 4.0): read Wikipedia sentences with exact transcripts. Only the needed clips
  are downloaded, into `~/.cache/hikaya-eval`. Nothing is committed.
- **Models:** catalog ids from `src-tauri/src/catalog/catalog.json`; their Q8_0 file is downloaded
  once.
- **Scoring:** lowercase, punctuation ignored, accents kept. Numbers written as digits versus
  words count as errors, as they do for a user.
- **Language:** `--spoken auto` (default) lets the app detect the language, like a user who
  speaks both; `given` sets it, which shows the best case.
- Standard library only. Runs the same on the RTX laptop; pass `--bin` with the path to the
  Windows build.

MINDS-14 (phone recordings) was tried first and dropped: its references often stop partway
through what the caller said, so a model that transcribed everything looked wrong.

Results are recorded in [`docs/hikaya/BASELINE.md`](../../docs/hikaya/BASELINE.md).
