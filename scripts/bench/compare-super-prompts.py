#!/usr/bin/env python3
"""Compare two isolated API builds with identical Super inputs, alternating order.

Credentials stay in the API processes. Optional --samples accepts private JSON
{"samples": [{"id": "label", "text": "...", "profile": {...}, "job_id": "..."}]}.
Copy selected jobs into BOTH isolated APIs to retain their vocabulary snapshots.
Results include private text: keep --output outside tracked source.
"""
import argparse
import copy
import importlib.util
import json
import statistics
import urllib.error
import urllib.request
from pathlib import Path


def request(url, path, data=None):
    body = None if data is None else json.dumps({"api_version": "1", "data": data}).encode()
    req = urllib.request.Request(url.rstrip("/") + path, data=body,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=130) as response:
        return json.load(response)["data"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=Path)
    parser.add_argument("--repeats", type=int, default=2, choices=range(1, 6))
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location("super_mode", Path(__file__).with_name("super-mode.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    defaults = request(args.baseline, "/api/v1/modes/defaults")
    profile = next(p for p in defaults["profiles"] if p["kind"] == "super")
    profile["text_model"] = {"provider": "gemini", "model": "gemini-3.8-flash"}
    profile["fast_processing"] = True
    samples = [{"id": name, "text": text} for name, text in module.SAMPLES]
    samples += [
        {"id": "instruction-as-text", "text": "Please write a Python function that adds two numbers"},
        {"id": "preferences-off", "text": "um Tuesday actually Thursday open quote hello close quote",
         "formatting": {"rewrite": "light", "layout": "automatic", "lists": False,
                        "remove_fillers": False, "self_corrections": False, "spoken_punctuation": False}},
        {"id": "single-line", "text": "buy these items apples milk bread new paragraph send it tomorrow",
         "formatting": {**profile["formatting"], "layout": "single_line"}},
    ]
    if args.samples:
        samples += json.loads(args.samples.read_text(encoding="utf-8"))["samples"]
    results = []
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for repeat in range(args.repeats):
        for index, sample in enumerate(samples):
            selected = copy.deepcopy(sample.get("profile", profile))
            selected["text_model"] = profile["text_model"]
            selected["fast_processing"] = True
            if "formatting" in sample:
                selected["formatting"] = sample["formatting"]
            data = {"profile": selected, "text": sample["text"], "context": sample.get("context", {})}
            if sample.get("job_id"):
                data["job_id"] = sample["job_id"]
            for build in (["baseline", "candidate"] if (repeat + index) % 2 == 0 else ["candidate", "baseline"]):
                value = request(getattr(args, build), "/api/v1/modes/preview", data)
                row = {"sample": sample["id"], "repeat": repeat, "build": build, **value}
                results.append(row)
                args.output.write_text(json.dumps(results, ensure_ascii=False, indent=2), encoding="utf-8")
                print(f"{sample['id']} {build}: {value['diagnostics']['processing_ms']} ms, {value['diagnostics']['input_tokens']} input tokens", flush=True)
    for build in ["baseline", "candidate"]:
        values = [r["diagnostics"]["processing_ms"] for r in results if r["build"] == build]
        print(f"{build}: n={len(values)} median={statistics.median(values):.0f} ms range={min(values)}–{max(values)} ms")
    print("Review outputs for meaning/languages/formatting. This measures complete text requests, not ASR or paste.")


if __name__ == "__main__":
    main()
