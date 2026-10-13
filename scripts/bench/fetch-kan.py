#!/usr/bin/env python3
"""Freeze the five MW-11 public Hebrew validation rows privately; no inference.

This does not grant a redistribution license. Explicitly confirm authorized private
corpus reuse; keep downloaded media, references and provider URLs outside source.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import urllib.request
import wave


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--public-corpus-reuse-confirmed", action="store_true", required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    ffmpeg_version = subprocess.check_output([args.ffmpeg, "-version"], text=True).splitlines()[0]
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    if output.is_relative_to(root):
        parser.error("Keep corpus outside source")
    output.mkdir(parents=True, exist_ok=False)
    url = "https://datasets-server.huggingface.co/rows?dataset=imvladikon%2Fhebrew_speech_kan&config=default&split=validation&offset=0&length=5"
    with urllib.request.urlopen(url, timeout=60) as response:
        response_bytes = response.read()
    (output / "dataset-response.json").write_bytes(response_bytes)
    rows = json.loads(response_bytes)["rows"]
    if [row["row_idx"] for row in rows] != list(range(5)):
        raise ValueError("Expected the frozen selection of validation rows 0-4")
    samples = []
    for row in rows:
        index, data = row["row_idx"], row["row"]
        audio = data["audio"]
        source_url = audio[0]["src"] if isinstance(audio, list) else audio["src"]
        original, prepared = output / f"kan-{index:03d}.source", output / f"kan-{index:03d}.wav"
        urllib.request.urlretrieve(source_url, original)
        subprocess.run([args.ffmpeg, "-nostdin", "-v", "error", "-i", str(original), "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(prepared)], check=True)
        with wave.open(str(prepared)) as wav:
            duration = wav.getnframes() / 16
        samples.append({"id": f"kan-{index:03d}", "input_language": "he", "expected_text": data["sentence"],
                        "audio_path": str(prepared), "audio_sha256": hashlib.sha256(prepared.read_bytes()).hexdigest(),
                        "source_audio_sha256": hashlib.sha256(original.read_bytes()).hexdigest(), "duration_ms": duration})
        print("Prepared public Hebrew row", index, flush=True)
    manifest = {"public_corpus_reuse_confirmed": True,
                "source": "imvladikon/hebrew_speech_kan validation rows 0-4; existing MW-11 authorized private reuse; no inferred redistribution license",
                "dataset_response_sha256": hashlib.sha256(response_bytes).hexdigest(),
                "preparation": {"codec": "pcm_s16le", "sample_rate": 16000, "channels": 1,
                                "ffmpeg_version": ffmpeg_version},
                "samples": samples}
    (output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print("Frozen", len(samples), "public Hebrew clips")


if __name__ == "__main__":
    main()
