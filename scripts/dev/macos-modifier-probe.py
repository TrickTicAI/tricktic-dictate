#!/usr/bin/env python3
"""Sample only Mac modifier state and allowlisted Writer delivery timings.

Run outside a filesystem/window-server sandbox. No event tap, character keys,
clipboard reads, audio, transcript text, application names or window titles.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import sys
import time

MODIFIER_MASK = sum(1 << bit for bit in (17, 18, 19, 20, 23))
MODIFIER_KEYS = (54, 55, 56, 60, 59, 62, 58, 61, 63)
STAGES = {
    "stop_accepted", "delivery_started", "clipboard_written", "keys_released",
    "modifier_wait_finished", "foreground_ready", "paste_sent", "delivery_finished",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New JSONL file outside source")
    parser.add_argument("--seconds", type=int, default=600)
    parser.add_argument("--delivery", type=Path, default=Path.home() / "Library/Application Support/MetrixWriter/State/last-delivery.json")
    args = parser.parse_args()
    if sys.platform != "darwin" or not 1 <= args.seconds <= 3600:
        parser.error("Requires macOS and a duration of 1–3600 seconds")
    cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    cg.CGEventSourceFlagsState.argtypes = [ctypes.c_int32]
    cg.CGEventSourceFlagsState.restype = ctypes.c_uint64
    cg.CGEventSourceKeyState.argtypes = [ctypes.c_int32, ctypes.c_uint16]
    cg.CGEventSourceKeyState.restype = ctypes.c_bool
    # Exclusive creation preserves previous receipts; private mode protects local diagnostics.
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", buffering=1) as log:
        start = time.monotonic()
        previous, stamp = None, None
        print(f"Sampling modifiers for {args.seconds}s → {args.output}", flush=True)
        try:
            while time.monotonic() - start < args.seconds:
                row = {"elapsed_ms": round((time.monotonic() - start) * 1000), "unix_ms": round(time.time() * 1000)}
                state = {}
                for name, source in (("session", 0), ("hardware", 1)):
                    state[f"{name}_flags"] = cg.CGEventSourceFlagsState(source) & MODIFIER_MASK
                    state[f"{name}_modifier_keys"] = [key for key in MODIFIER_KEYS if cg.CGEventSourceKeyState(source, key)]
                if state != previous:
                    log.write(json.dumps(dict(row, **state)) + "\n")
                    previous = state
                try:
                    next_stamp = args.delivery.stat().st_mtime_ns
                    if next_stamp != stamp:
                        receipt = json.loads(args.delivery.read_text())
                        delivery = {key: receipt.get(key) for key in ("copied", "pasted", "stop_to_delivery_ms")}
                        delivery["events"] = [
                            {"stage": event["stage"], "elapsed_ms": event.get("elapsed_ms")}
                            for event in (receipt.get("stages") or {}).get("events", [])
                            if event.get("stage") in STAGES
                        ]
                        log.write(json.dumps(dict(row, delivery=delivery, initial_receipt=stamp is None)) + "\n")
                        stamp = next_stamp
                except (OSError, ValueError):
                    pass
                time.sleep(0.01)
        except KeyboardInterrupt:
            pass
        print("Probe stopped; receipt flushed.")


if __name__ == "__main__":
    main()
