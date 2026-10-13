#!/usr/bin/env python3
"""Compatibility entry: run a bounded private Whisper smoke, then stop its workers.
Unlike the historical unauthenticated launcher, this never leaves a raw listener running.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--runtime", type=Path, required=True)
    p.add_argument("--model", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--backend", choices=["metal", "cpu"], required=True)
    p.add_argument("--language", default="he")
    p.add_argument("--threads", type=int)
    args = p.parse_args()
    runtime, model = args.runtime.resolve(), args.model.resolve()
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    threads = args.threads or (min(os.cpu_count() or 1, 16) if args.backend == "cpu" else 1)
    command = [str(runtime / "whisper-server"), "-m", model.name, "-l", args.language,
               "-t", str(threads), "-nt", "-ac", "768", "--port", "0"]
    if args.backend == "cpu":
        command.append("-ng")
    spec = importlib.util.spec_from_file_location("worker_smoke", Path(__file__).with_name("worker-auth-smoke.py"))
    smoke = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(smoke)
    os.chdir(model.parent)
    first = smoke.run(command, "whisper", backend=args.backend)
    smoke.run(command, "whisper", first, backend=args.backend)
    receipt = {"backend": args.backend, "private_worker_smoke": "passed", "launches": 2, "workers_stopped": True}
    (output / "ready.json").write_text(json.dumps(receipt) + "\n")
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
