#!/usr/bin/env python3
"""Regression test for the dictation cleanup prompt.

Sends each case in cases.json through an OpenAI-compatible endpoint (Ollama by
default) the same way the app does, then checks that the phrases in "keep"
survived and none of the phrases in "drop" appeared. The main thing it guards
is issue #6: mixed English/French must never be translated.

    python3 eval/cleanup/run.py                        # Hikaya prompt, local Ollama
    python3 eval/cleanup/run.py --prompt path/to.txt   # any other prompt
    python3 eval/cleanup/run.py --model qwen3:8b --base-url http://host:11434/v1

Standard library only. Exit code 1 if any case fails.
"""
import argparse
import json
import pathlib
import sys
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_PROMPT = ROOT / "src-tauri/src/hikaya/cleanup_prompt.txt"


def complete(base_url, model, content, timeout):
    body = json.dumps(
        {
            "model": model,
            "temperature": 0,
            "messages": [{"role": "user", "content": content}],
        }
    ).encode()
    request = urllib.request.Request(
        f"{base_url.rstrip('/')}/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)["choices"][0]["message"]["content"].strip()


def check(case, output):
    folded = output.casefold().replace("’", "'")
    missing = [p for p in case["keep"] if p.casefold() not in folded]
    # Case matters here: a terminal command must not come back capitalized.
    missing += [p for p in case.get("keep_exact", []) if p not in output]
    leaked = [p for p in case["drop"] if p.casefold() in folded]
    return missing, leaked


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--prompt", type=pathlib.Path, default=DEFAULT_PROMPT)
    parser.add_argument("--cases", type=pathlib.Path, default=pathlib.Path(__file__).with_name("cases.json"))
    parser.add_argument("--model", default="qwen3:4b-instruct")
    parser.add_argument("--base-url", default="http://127.0.0.1:11434/v1")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--json", type=pathlib.Path, help="also write the results to this file")
    args = parser.parse_args()

    template = args.prompt.read_text(encoding="utf-8")
    cases = json.loads(args.cases.read_text(encoding="utf-8"))
    results, failures = [], 0
    for case in cases:
        started = time.monotonic()
        try:
            vocabulary = ", ".join(case.get("vocabulary", [])) or "(none)"
            # As the app does: the line about the target app, or nothing.
            context = case.get("context", "")
            template_ctx = template.replace("${context}", context) if context else template.replace("${context}\n\n", "").replace("${context}", "")
            prompt = template_ctx.replace("${vocabulary}", vocabulary).replace("${output}", case["input"])
            output = complete(args.base_url, args.model, prompt, args.timeout)
            missing, leaked = check(case, output)
        except Exception as error:  # network or model failure counts as a failed case
            output, missing, leaked = f"<error: {error}>", ["(no answer)"], []
        seconds = time.monotonic() - started
        ok = not missing and not leaked
        failures += not ok
        results.append({"id": case["id"], "ok": ok, "seconds": round(seconds, 1), "output": output, "missing": missing, "leaked": leaked})
        print(f"{'PASS' if ok else 'FAIL'}  {case['id']:<26} {seconds:5.1f}s  {output}")
        if missing:
            print(f"      missing: {missing}")
        if leaked:
            print(f"      must not appear: {leaked}")

    print(f"\n{len(cases) - failures}/{len(cases)} passed with {args.model} and {args.prompt.name}")
    if args.json:
        args.json.write_text(json.dumps({"model": args.model, "prompt": str(args.prompt), "results": results}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
