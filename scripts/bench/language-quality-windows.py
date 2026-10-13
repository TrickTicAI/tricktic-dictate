#!/usr/bin/env python3
"""Run supplied public/consented fixtures on an owned authenticated Windows Vulkan worker.

Run with native Windows Python. No application profile changes or shared worker.
Model/audio are read-only. Private run files retain predictions; stdout is counts only.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time
import urllib.request
import uuid
import wave

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/evaluate-language"))
from benchmark import archive, encoded, private_path  # noqa: E402


def file_hash(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def execute(args):
    if os.name != "nt":
        raise ValueError("Use native Windows Python and a Windows Vulkan worker; server CPU inference is forbidden")
    fixture_path = args.fixtures.resolve()
    fixture = json.loads(fixture_path.read_bytes())
    selected = set(args.ids.split(",")) if args.ids else None
    samples = [s for s in fixture["samples"] if (selected is None and "audio_path" in s) or (selected is not None and s["id"] in selected)]
    if not samples or (selected is not None and selected != {s["id"] for s in samples}):
        raise ValueError("Select existing fixtures with actual audio paths")
    variants = json.loads(args.variants.read_bytes()) if args.variants else {}
    if set(variants) - {s["id"] for s in fixture["samples"]}:
        raise ValueError("Unknown audio variant fixture id")
    prepared = []
    for sample in samples:
        if sample["id"] in variants:
            variant = variants[sample["id"]]
            if not variant.get("preparation"):
                raise ValueError("Variant requires preparation metadata")
            if file_hash(private_path(sample["audio_path"])) != sample["audio_sha256"]:
                raise ValueError("Variant source does not match frozen fixture")
            sample = {**sample, **{k: variant[k] for k in ("audio_path", "audio_sha256", "preparation")}, "source_audio_path": sample["audio_path"],
                      "source_audio_sha256": sample["audio_sha256"]}
        prepared.append(sample)
    samples = prepared
    inputs = {}
    for sample in samples:
        audio = private_path(sample["audio_path"])
        if sample.get("requires_private_reference") or not sample.get("expected_text"):
            raise ValueError("A complete reference is required")
        if sample["split"] == "public":
            if fixture.get("public_corpus_reuse_confirmed") is not True:
                raise ValueError("Confirm public reuse in the private manifest")
        elif fixture.get("consent_confirmed") is not True:
            raise ValueError("Confirm consent for recorded synthetic scripts")
        inputs[sample["id"]] = file_hash(audio)
        if sample.get("audio_sha256") != inputs[sample["id"]]:
            raise ValueError("Supply the frozen audio hash")
        with wave.open(str(audio)) as wav:
            if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, 16000):
                raise ValueError("Runner accepts prepared 16 kHz mono PCM WAV only")
    output = private_path(args.output)
    output.mkdir(parents=True, exist_ok=False)
    binary, model = args.worker.resolve(), args.model.resolve()
    dlls = {p.name: file_hash(p) for p in sorted(binary.parent.glob("*.dll"))}
    decoding = {"best_of": args.best_of, "beam_size": args.beam_size, "temperature": 0,
                "temperature_inc": 0, "audio_ctx": 0, "translate": "false", "no_timestamps": "true",
                "response_format": "json", "language": "per_fixture; mixed=auto", "threads": 1}
    model_hash = file_hash(model)
    stage = {"provider": "local", "model": model.name, "model_revision": model_hash,
             "prompt": args.prompt, "decoding": decoding}
    run = {"kind": "audio_run", "fixture_version": fixture["fixture_version"],
           "model": model.name, "model_revision": model_hash,
           "run_at": datetime.now(timezone.utc).isoformat(), "settings": decoding,
           "device": "windows_gpu", "hardware": args.hardware,
           "runtime": {"worker_sha256": file_hash(binary), "dll_sha256": dlls, "backend": "Vulkan",
                       "runner_sha256": file_hash(__file__)},
           "stages": {"raw": stage}, "results": []}
    capability, instance = secrets.token_hex(32), secrets.token_hex(32)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    log_path = output / "worker.log"
    command = [str(binary), "-m", str(model), "-l", "auto", "-t", "1", "-nt", "--port", "0"]
    with log_path.open("wb") as log:
        child = subprocess.Popen(command, cwd=binary.parent, stdin=subprocess.PIPE,
                                 stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            child.stdin.write((capability + "\n" + instance + "\n").encode()); child.stdin.close()
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError("Owned worker exited before readiness; inspect private log")
                text = log_path.read_text(encoding="utf-8", errors="replace")
                found = re.search(r"writer: private listener (\d+)", text)
                if found:
                    address = "http://127.0.0.1:" + found.group(1)
                    with opener.open(urllib.request.Request(address + "/health", headers={"Authorization": "Bearer " + capability}), timeout=5) as response:
                        health = json.load(response)
                        if response.headers.get("X-Writer-Instance") != instance:
                            raise RuntimeError("Wrong private worker instance")
                    if health.get("warm") is True and health.get("writer_protocol") == 1:
                        if "using Vulkan" not in text:
                            raise RuntimeError("GPU backend not proved; refusing speech inference")
                        run["hardware"] = {"declared": args.hardware,
                                           "gpu_log": [line for line in text.splitlines() if line.startswith("ggml_vulkan:")]}
                        break
                time.sleep(.1)
            else:
                raise RuntimeError("Worker warmup timed out")
            for sample in samples:
                boundary = uuid.uuid4().hex
                parameters = {**decoding, "prompt": args.prompt,
                              "language": "auto" if sample["input_language"] == "mixed" else sample["input_language"]}
                parameters.pop("threads")
                parts = [f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode() for k, v in parameters.items()]
                parts += [f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="speech.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode(),
                          Path(sample["audio_path"]).read_bytes(), f"\r\n--{boundary}--\r\n".encode()]
                request = urllib.request.Request(address + "/inference", data=b"".join(parts),
                    headers={"Authorization": "Bearer " + capability, "Content-Type": "multipart/form-data; boundary=" + boundary})
                start = time.perf_counter()
                with opener.open(request, timeout=600) as response:
                    payload = json.load(response)
                    if response.headers.get("X-Writer-Instance") != instance:
                        raise RuntimeError("Inference response instance mismatch")
                elapsed = (time.perf_counter() - start) * 1000
                if not isinstance(payload.get("text"), str):
                    raise ValueError("Missing actual worker prediction")
                with wave.open(sample["audio_path"]) as wav:
                    duration = wav.getnframes() / wav.getframerate() * 1000
                row = {"id": sample["id"], "input_language": sample["input_language"], "raw_text": payload["text"],
                       "audio_path": sample["audio_path"], "audio_sha256": inputs[sample["id"]],
                       "codec": "pcm_s16le/16000/mono", "recording_duration_ms": duration,
                       "latency_ms": {"raw": elapsed}, "parameters": parameters,
                       "public_corpus_reuse_confirmed": sample["split"] == "public",
                       "consent_confirmed": fixture.get("consent_confirmed") is True}
                if "preparation" in sample:
                    row.update({k: sample[k] for k in ("source_audio_path", "source_audio_sha256", "preparation")})
                run["results"].append(row)
                # Partial attempts stay private; only a complete validated run is archived.
                (output / "attempt.json").write_bytes(encoded(run))
                print("Completed fixture", len(run["results"]), "of", len(samples), flush=True)
        finally:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill(); child.wait()
    logs = log_path.read_bytes()
    if capability.encode() in logs or instance.encode() in logs:
        log_path.unlink()
        raise RuntimeError("Worker leaked handshake into log; log removed")
    if inputs != {s["id"]: file_hash(s["audio_path"]) for s in samples}:
        raise ValueError("Source audio changed during inference")
    run_path = output / "run.json"
    run_path.write_bytes(encoded(run))
    target = archive(fixture_path, run_path, output.parent / "archives")
    (output / "receipt.json").write_bytes(encoded({"archive": str(target), "worker_stopped": True,
                                                   "backend_proved": "Vulkan", "samples": len(samples),
                                                   "latency_scope": "warm worker HTTP inference; excludes capture, application queues, cleanup and insertion"}))
    print("Archived", target.name, "with", len(samples), "raw-ASR fixtures; cleanup unmeasured")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--hardware", required=True)
    parser.add_argument("--ids")
    parser.add_argument("--variants", type=Path, help="JSON map of fixture id to prepared audio path/hash/preparation; base fixtures stay unchanged")
    parser.add_argument("--prompt", default="")
    parser.add_argument("--best-of", type=int, default=1)
    parser.add_argument("--beam-size", type=int, default=1)
    args = parser.parse_args()
    if args.best_of < 1 or args.beam_size < 1:
        parser.error("Decoding counts must be positive")
    execute(args)


if __name__ == "__main__":
    main()
