"""Command-mode regression set: each case is a selection, a spoken instruction, phrases the
result must keep and phrases it must not contain. Runs the app's prompt
(crates/hikaya-core/src/prompts/command.txt) against an OpenAI-compatible endpoint, Ollama by
default, and cleans the reply as the app does (a fence or quotes around the whole text go).

    python3 eval/command/run.py [--model qwen3:4b-instruct] [--base-url http://127.0.0.1:11434/v1]
"""

import argparse
import json
import pathlib
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "eval" / "cleanup"))
from run import complete  # noqa: E402  (same request as the cleanup eval)

PROMPT = ROOT / "crates/hikaya-core/src/prompts/command.txt"


def clean(reply):
    text = reply.split("</think>")[-1].strip() if reply.lstrip().startswith("<think>") else reply.strip()
    if text.startswith("```"):
        text = text.strip("`")
        first, _, rest = text.partition("\n")
        text = (rest if " " not in first.strip() else text).strip()
    for o, c in (('"', '"'), ("“", "”"), ("«", "»")):
        if text.startswith(o) and text.endswith(c) and c not in text[1:-1]:
            text = text[1:-1].strip()
    return text


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="qwen3:4b-instruct")
    parser.add_argument("--base-url", default="http://127.0.0.1:11434/v1")
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    template = PROMPT.read_text()
    cases = json.loads((pathlib.Path(__file__).with_name("cases.json")).read_text())
    passed = 0
    for case in cases:
        prompt = (template.replace("${context}", "").replace("${instruction}", case["instruction"])
                  .replace("${selection}", case["selection"]))
        started = time.time()
        try:
            out = clean(complete(args.base_url, args.model, prompt, args.timeout))
        except Exception as error:  # report and go on
            out = f"<error: {error}>"
        folded = out.casefold().replace("’", "'")
        missing = [p for p in case["keep"] if p.casefold() not in folded]
        leaked = [p for p in case["drop"] if p.casefold() in folded]
        long = "max_ratio" in case and len(out) > case["max_ratio"] * len(case["selection"])
        ok = not missing and not leaked and not long
        passed += ok
        print(f"{'PASS' if ok else 'FAIL'}  {case['id']:<16} {time.time() - started:5.1f}s  {out!r}")
        if not ok:
            print(f"      missing: {missing} leaked: {leaked}{' too long' if long else ''}")
    print(f"\n{passed}/{len(cases)} passed with {args.model}")


if __name__ == "__main__":
    main()
