# Speaker diarization

Measures how well a diarization pipeline tells speakers apart, for
[ADR 0005](../../docs/adr/0005-telling-speakers-apart.md). Uses meetings of the AMI corpus (CC BY
4.0) and the word-aligned references published by pyannote; both are downloaded to
`~/.cache/hikaya-eval/diarization`, never into the repository.

```bash
uv run --python 3.12 --with sherpa-onnx --with soundfile --with numpy --with pyannote.metrics \
  python eval/diarization/run.py --meetings ES2004a IS1009a --thresholds 0.9 1.0
```

`run.py` measures sherpa-onnx. `speakrs/` is a small program that runs the pyannote community-1
pipeline natively (no Python, no ONNX Runtime) and prints RTTM; `score.py` scores it:

```bash
cd eval/diarization/speakrs
cargo run --release -- ~/.cache/hikaya-eval/diarization/ES2004a.wav ES2004a > ES2004a.rttm
# On the laptop's RTX 4070 (needs the NVIDIA driver; CUDA libraries load at run time):
cargo run --release --no-default-features --features cuda -- ES2004a.wav ES2004a > ES2004a.rttm
cd ../../.. && uv run --python 3.12 --with pyannote.metrics python eval/diarization/score.py \
  ~/.cache/hikaya-eval/diarization/ES2004a.rttm eval/diarization/speakrs/ES2004a.rttm
```

The audio is fetched by `run.py` on its first run. Results are recorded in the ADR.
