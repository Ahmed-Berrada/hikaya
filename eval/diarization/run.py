#!/usr/bin/env python3
"""Speaker diarization error rate (DER) for ADR 0005.

Runs sherpa-onnx's offline diarization (pyannote segmentation-3.0 plus a
speaker-embedding model) on meetings of the AMI corpus (University of
Edinburgh, CC BY 4.0: real meetings of four people, headset mix) and scores
it against the word-aligned references published by pyannote.

    uv run --python 3.12 --with sherpa-onnx --with soundfile --with numpy \\
        --with pyannote.metrics python eval/diarization/run.py
    ... python eval/diarization/run.py --meetings ES2004a IS1009a --thresholds 0.9 1.0

DER uses a 0.25 s collar and skips overlapped speech, as is usual for AMI.
"--speakers 4" gives the true count, as a best case for the clustering.

Audio, references and models are downloaded to the cache once, never into
the repository.
"""
import argparse
import os
import pathlib
import tarfile
import time
import urllib.request

import numpy as np
import sherpa_onnx
import soundfile as sf
from pyannote.core import Annotation, Segment
from pyannote.metrics.diarization import DiarizationErrorRate

CACHE = pathlib.Path(os.environ.get("HIKAYA_EVAL_CACHE", pathlib.Path.home() / ".cache/hikaya-eval")) / "diarization"
AMI = "https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{m}/audio/{m}.Mix-Headset.wav"
RTTM = "https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/main/only_words/rttms/test/{m}.rttm"
MODELS = "https://github.com/k2-fsa/sherpa-onnx/releases/download"
SEGMENTATION = f"{MODELS}/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2"
EMBEDDINGS = {
    "wespeaker-resnet34": f"{MODELS}/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx",
    "3dspeaker-campplus": f"{MODELS}/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
}


def fetch(url: str, path: pathlib.Path) -> pathlib.Path:
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(path.suffix + ".part")
        with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        tmp.rename(path)
    return path


def segmentation_model() -> pathlib.Path:
    model = CACHE / "sherpa-onnx-pyannote-segmentation-3-0" / "model.onnx"
    if not model.exists():
        archive = fetch(SEGMENTATION, CACHE / "segmentation.tar.bz2")
        with tarfile.open(archive) as tar:
            tar.extractall(CACHE, filter="data")
    return model


def reference(meeting: str) -> Annotation:
    ref = Annotation()
    for line in fetch(RTTM.format(m=meeting), CACHE / f"{meeting}.rttm").read_text().splitlines():
        f = line.split()
        ref[Segment(float(f[3]), float(f[3]) + float(f[4]))] = f[7]
    return ref


def diarize(audio: np.ndarray, embedding: pathlib.Path, threshold: float, speakers: int) -> Annotation:
    config = sherpa_onnx.OfflineSpeakerDiarizationConfig(
        segmentation=sherpa_onnx.OfflineSpeakerSegmentationModelConfig(
            pyannote=sherpa_onnx.OfflineSpeakerSegmentationPyannoteModelConfig(model=str(segmentation_model())),
            num_threads=4,
        ),
        embedding=sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=str(embedding), num_threads=4),
        clustering=sherpa_onnx.FastClusteringConfig(num_clusters=speakers, threshold=threshold),
        min_duration_on=0.3,
        min_duration_off=0.5,
    )
    hyp = Annotation()
    for r in sherpa_onnx.OfflineSpeakerDiarization(config).process(audio).sort_by_start_time():
        hyp[Segment(r.start, r.end)] = f"s{r.speaker}"
    return hyp


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--meetings", nargs="+", default=["ES2004a"])
    parser.add_argument("--embedding", choices=sorted(EMBEDDINGS), default="wespeaker-resnet34")
    parser.add_argument("--thresholds", nargs="+", type=float, default=[0.9, 1.0])
    parser.add_argument("--speakers", type=int, default=-1, help="the true count, or -1 to find it")
    args = parser.parse_args()

    embedding = fetch(EMBEDDINGS[args.embedding], CACHE / f"{args.embedding}.onnx")
    print("| Meeting | Embedding | Threshold | Speakers found / real | DER | Speed (x real time) |")
    print("|---|---|---|---|---|---|")
    for meeting in args.meetings:
        audio, rate = sf.read(fetch(AMI.format(m=meeting), CACHE / f"{meeting}.wav"), dtype="float32")
        if audio.ndim > 1:
            audio = audio.mean(axis=1)
        assert rate == 16000, f"{meeting} is {rate} Hz"
        ref = reference(meeting)
        for threshold in args.thresholds:
            start = time.time()
            hyp = diarize(audio, embedding, threshold, args.speakers)
            took = time.time() - start
            der = DiarizationErrorRate(collar=0.25, skip_overlap=True)(ref, hyp)
            # Speakers holding at least 5% of the speech; the rest are fragments.
            total = sum(hyp.label_duration(l) for l in hyp.labels())
            main_speakers = sum(hyp.label_duration(l) >= 0.05 * total for l in hyp.labels())
            print(
                f"| {meeting} | {args.embedding} | {threshold if args.speakers < 0 else f'{args.speakers} given'} "
                f"| {main_speakers} ({len(hyp.labels())} with fragments) / {len(ref.labels())} "
                f"| {der:.1%} | {len(audio) / 16000 / took:.0f} |"
            )


if __name__ == "__main__":
    main()
