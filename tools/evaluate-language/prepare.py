#!/usr/bin/env python3
"""Freeze public corpus references or make deterministic PCM noise variants privately."""
import argparse
from array import array
import json
import math
from pathlib import Path
import random
import sys
import wave

from benchmark import digest, encoded, private_path


def import_public(fixtures, public, source_id):
    manifest = json.loads(Path(fixtures).read_bytes())
    source_bytes = Path(public).read_bytes()
    corpus = json.loads(source_bytes)
    if corpus.get("public_corpus_reuse_confirmed") is not True or not corpus.get("samples"):
        raise ValueError("explicit public corpus reuse and nonempty samples required")
    declared = next(s for s in manifest["public_sources"] if s["id"] == source_id)
    if declared.get("status") == "unmeasured":
        raise ValueError("source has no qualified public corpus")
    seen = {s["id"] for s in manifest["samples"]}
    for sample in corpus["samples"]:
        path = private_path(sample["audio_path"])
        sha = digest(path.read_bytes())
        if sample.get("audio_sha256", sha) != sha:
            raise ValueError("public audio hash mismatch")
        if sample["id"] in seen or not sample.get("expected_text") or sample["input_language"] != declared["language"]:
            raise ValueError("public sample requires unique id, complete reference and matching language")
        seen.add(sample["id"])
        manifest["samples"].append({**sample, "audio_path": str(path), "audio_sha256": sha,
                                    "split": "public", "tags": ["public_read_speech", "short_dictation"],
                                    "source_id": source_id})
    manifest.setdefault("public_imports", []).append({"source_id": source_id, "manifest_sha256": digest(source_bytes),
                                                      "provenance": corpus.get("provenance", corpus.get("source")),
                                                      "preparation": corpus.get("preparation"),
                                                      "dataset_response_sha256": corpus.get("dataset_response_sha256")})
    manifest["fixture_version"] += "+" + source_id + "-" + digest(source_bytes)[:12]
    return manifest


def noise(source, output, seed=108, snr_db=10):
    source, output = private_path(source), private_path(output)
    if not math.isfinite(snr_db):
        raise ValueError("SNR must be finite")
    with wave.open(str(source), "rb") as wav:
        params = wav.getparams()
        if (params.nchannels, params.sampwidth, params.framerate, params.comptype) != (1, 2, 16000, "NONE"):
            raise ValueError("noise input requires mono 16 kHz signed 16-bit PCM WAV")
        pcm = array("h", wav.readframes(params.nframes))
    if sys.byteorder != "little":
        pcm.byteswap()
    if not pcm or not any(pcm):
        raise ValueError("noise input requires nonsilent audio")
    rng = random.Random(seed)
    random_values = [rng.uniform(-1, 1) for _ in pcm]
    rms = math.sqrt(sum(x * x for x in pcm) / len(pcm))
    noise_rms = math.sqrt(sum(x * x for x in random_values) / len(pcm))
    scale = rms / (10 ** (snr_db / 20)) / noise_rms
    mixed = [round(x + n * scale) for x, n in zip(pcm, random_values)]
    clipped = sum(x < -32768 or x > 32767 for x in mixed)
    if clipped:
        raise ValueError("noise would clip; attenuate source explicitly and retain that preparation")
    result = array("h", mixed)
    if sys.byteorder != "little":
        result.byteswap()
    with output.open("xb") as file:
        with wave.open(file, "wb") as wav:
            wav.setparams(params)
            wav.writeframes(result.tobytes())
    return {"source_audio_path": str(source), "source_audio_sha256": digest(source.read_bytes()),
            "audio_path": str(output), "audio_sha256": digest(output.read_bytes()),
            "preparation": {"operation": "additive_uniform_noise", "seed": seed, "snr_db": snr_db,
                            "algorithm": "mw108-pcm-noise-v1", "clipped_samples": 0}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    public = sub.add_parser("public")
    public.add_argument("--fixtures", type=Path, required=True)
    public.add_argument("--public-manifest", type=Path, required=True)
    public.add_argument("--source", choices=["librispeech", "kan"], required=True)
    public.add_argument("--output", type=Path, required=True)
    variant = sub.add_parser("noise")
    variant.add_argument("source", type=Path)
    variant.add_argument("--output", type=Path, required=True)
    variant.add_argument("--seed", type=int, default=108)
    variant.add_argument("--snr-db", type=float, default=10)
    args = parser.parse_args()
    if args.action == "public":
        result = import_public(args.fixtures, args.public_manifest, args.source)
        with private_path(args.output).open("xb") as output:
            output.write(encoded(result))
        print(args.output)
    else:
        print(json.dumps(noise(args.source, args.output, args.seed, args.snr_db)))


if __name__ == "__main__":
    main()
