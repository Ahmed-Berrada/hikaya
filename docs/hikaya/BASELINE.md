# Speech model baseline

How well each speech model transcribes French and English, measured with
[`eval/wer/run.py`](../../eval/wer/README.md) through the real app.

## Home server, CPU (2026-11-16, Parakeet Ultra added 2026-11-17)

Intel i7-10510U (4 cores), 24 GB RAM, models in Q8_0. The first 40 clips of the FLEURS test set per
language (read Wikipedia sentences). WER: lower is better. Speed: seconds of audio transcribed per
second; above 1 is faster than real time. Language left on **auto**, as for a user who speaks both.

| Model                  | Size   | French WER | English WER | Speed    | Notes                                       |
| ---------------------- | ------ | ---------- | ----------- | -------- | ------------------------------------------- |
| Parakeet Ultra         | 740 MB | **5.1%**   | 4.6%        | 4.6×     | **Default**: best accuracy for the speed    |
| Parakeet TDT 0.6B v3   | 740 MB | 5.6%       | 5.1%        | 4.8×     | Close second                                |
| Qwen3-ASR 0.6B         | 850 MB | 9.0%       | **3.9%**    | 1.6×     | Best English, weaker French                 |
| Whisper large-v3-turbo | 886 MB | 9.2%       | 4.2%        | 0.2–0.4× | 10 clips only; on CPU (see below)           |
| Whisper small          | 270 MB | 13.7%      | 7.1%        | 1.5×     |                                             |
| Whisper base           | 85 MB  | 28.4%      | 10.4%       | 4.6×     | Too weak for French                         |
| Canary 180M Flash      | 218 MB | 94.6%      | 10.7%       | 6.6×     | Needs the language detector on auto (below) |

### Findings

- **Canary translates French on "auto" without the language detector.** Canary cannot tell the
  language itself; Handy covers that with a separate, downloadable language detector, which the
  test setup did not have. Without it, Canary writes English, so French comes out translated. Told
  the language is French, it scores **7.8%** at 5.2×. So Canary is only safe for a bilingual user
  once the detector is installed; Hikaya should not recommend it otherwise.
- **Whisper large-v3-turbo produced gibberish on the home server's Intel GPU** (Vulkan on Intel
  UHD 620): over 100% WER, such as "visit your for". On the CPU of the same machine it is accurate.
  The RTX laptop uses a different GPU and driver; check it there before trusting it.
- FLEURS clips are a single language each. Sentences that switch between English and French are
  not measured yet; the cleanup prompt's evaluation covers mixed text, not mixed audio.

### Recommendation

- **Default for an English + French speaker: Parakeet Ultra.** The most accurate in French and
  close to the best in English, at the same speed and size as Parakeet TDT 0.6B v3, and it detects
  the language itself. It is also what the app's setup already recommends for these two languages
  (the model package's "best" pick), so new installs land on it without any change.
- **For English-heavy use:** Qwen3-ASR 0.6B.
- **On a strong GPU:** compare Whisper large-v3-turbo and Parakeet Ultra on the laptop before
  choosing.
- Accuracy keeps improving with use through the learning loop (rules and vocabulary), which
  works with any model.

## RTX 4070 laptop

Not measured yet. On Windows, from the repository root, after building the app:

```powershell
python eval/wer/run.py --bin src-tauri/target/release/handy.exe --models parakeet-tdt-0.6b-v3 whisper-large-v3-turbo Qwen3-ASR-0.6B --samples 100
```
