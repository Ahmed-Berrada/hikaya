#!/usr/bin/env python3
"""Scores a diarization (RTTM) against an AMI reference, for ADR 0005.

    uv run --python 3.12 --with pyannote.metrics python eval/diarization/score.py \\
        ~/.cache/hikaya-eval/diarization/ES2004a.rttm ES2004a.rttm

DER with a 0.25 s collar, overlapped speech skipped, as in run.py.
"""
import sys

from pyannote.core import Annotation, Segment
from pyannote.metrics.diarization import DiarizationErrorRate


def load(path: str) -> Annotation:
    annotation = Annotation()
    for line in open(path, encoding="utf-8"):
        f = line.split()
        if f and f[0] == "SPEAKER":
            annotation[Segment(float(f[3]), float(f[3]) + float(f[4]))] = f[7]
    return annotation


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    ref, hyp = load(sys.argv[1]), load(sys.argv[2])
    der = DiarizationErrorRate(collar=0.25, skip_overlap=True)(ref, hyp)
    total = sum(hyp.label_duration(label) for label in hyp.labels())
    main_speakers = sum(hyp.label_duration(label) >= 0.05 * total for label in hyp.labels())
    print(
        f"DER {der:.1%}; {main_speakers} speakers "
        f"({len(hyp.labels())} with fragments) / {len(ref.labels())} real"
    )


if __name__ == "__main__":
    main()
