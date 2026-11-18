# Speaker diarization

Measures how well a diarization pipeline tells speakers apart, for
[ADR 0005](../../docs/adr/0005-telling-speakers-apart.md). Uses meetings of the AMI corpus (CC BY
4.0) and the word-aligned references published by pyannote; both are downloaded to
`~/.cache/hikaya-eval/diarization`, never into the repository.

```bash
uv run --python 3.12 --with sherpa-onnx --with soundfile --with numpy --with pyannote.metrics \
  python eval/diarization/run.py --meetings ES2004a IS1009a --thresholds 0.9 1.0
```

Results are recorded in the ADR.
