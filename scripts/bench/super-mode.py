#!/usr/bin/env python3
"""Compare Fast/Balanced Super using a running, isolated API with a configured key.

Uses synthetic text only, never records audio or changes active settings. Sends
12 cloud text requests by default. Credentials stay in the API process. Receipts
must be written outside tracked source (for example .local/super-results.json).
"""
import argparse
import json
import statistics
import urllib.request
from pathlib import Path

SAMPLES = [
    ("list", "Please buy these items apples pears rice and coffee"),
    ("punctuation", "She said open quote I will be there close quote open parentheses if the train is on time close parentheses dot dot dot we will see"),
    ("hebrew", "אני צריך לקנות את הדברים הבאים חלב לחם וביצים פסקה חדשה נקבע ליום שלישי בעצם ליום חמישי בשעה שלוש"),
    ("mixed", "תשלח את ה pull request מחר בעצם ביום חמישי ותגיד לדנה open quote ready for review close quote"),
    ("literal", "The app does not add new lines or quotation marks when I expect it to"),
    ("paragraphs", "The first issue is the microphone it disconnects whenever I plug in the headphones new paragraph the second issue is the settings page I want a save button that stays visible"),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Isolated Writer API URL")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--repeats", type=int, default=1, choices=range(1, 6))
    args = parser.parse_args()

    def request(path, data=None):
        body = None if data is None else json.dumps({"api_version": "1", "data": data}).encode()
        req = urllib.request.Request(args.url.rstrip("/") + path, data=body, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=130) as response:
            return json.load(response)["data"]

    defaults = request("/api/v1/modes/defaults")
    profile = next(p for p in defaults["profiles"] if p["kind"] == "super")
    profile["text_model"] = {"provider": "gemini", "model": "gemini-3.8-flash"}
    results = []
    for repeat in range(args.repeats):
        for name, text in SAMPLES:
            # Alternate order on repeats to reduce systematic warm-connection bias.
            for fast in ([False, True] if repeat % 2 == 0 else [True, False]):
                profile["fast_processing"] = fast
                value = request("/api/v1/modes/preview", {"profile": profile, "text": text, "context": {}})
                results.append({"sample": name, "fast": fast, "repeat": repeat, **value})
                print(f"{name} {'fast' if fast else 'balanced'}: {value['diagnostics']['processing_ms']} ms", flush=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(results, ensure_ascii=False, indent=2), encoding="utf-8")
    for fast in [False, True]:
        times = [r["diagnostics"]["processing_ms"] for r in results if r["fast"] == fast]
        print(f"{'Fast' if fast else 'Balanced'} median {statistics.median(times):.0f} ms, max {max(times)} ms")
    print("Review every output for meaning, formatting and preserved languages; timings alone do not qualify a model.")


if __name__ == "__main__":
    main()
