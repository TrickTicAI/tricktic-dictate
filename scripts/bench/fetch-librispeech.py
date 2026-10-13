#!/usr/bin/env python3
"""Fetch public LibriSpeech test-clean rows (CC BY 4.0) through the Hugging Face
datasets server into private storage as 16 kHz mono WAV, with a manifest that
scripts/bench/dictation-latency.py accepts. English read speech: a held-out
regression check, not accented dictation. Keep the output outside source."""
import argparse
import hashlib
import json
import os
import subprocess
import urllib.request
import wave
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--count', type=int, default=30)
    parser.add_argument('--max-seconds', type=float, default=15)
    parser.add_argument('--offsets', default='0,100,400,900,1500')
    parser.add_argument('--ffmpeg', default='ffmpeg')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    out = args.output.resolve()
    if out.is_relative_to(repo):
        parser.error('Keep corpus audio outside source')
    if args.count < 1 or args.max_seconds <= 0:
        parser.error('Supply positive count and max-seconds')
    ffmpeg_version = subprocess.check_output([args.ffmpeg, '-version'], text=True).splitlines()[0]
    out.mkdir(parents=True, exist_ok=False)
    samples = []
    responses = []
    for offset in args.offsets.split(','):
        url = f'https://datasets-server.huggingface.co/rows?dataset=openslr%2Flibrispeech_asr&config=clean&split=test&offset={int(offset)}&length=12'
        with urllib.request.urlopen(url, timeout=60) as response:
            response_bytes = response.read()
        (out / f'rows-{int(offset)}.json').write_bytes(response_bytes)
        responses.append(dict(offset=int(offset), sha256=hashlib.sha256(response_bytes).hexdigest()))
        rows = json.loads(response_bytes)['rows']
        for row in rows:
            r = row['row']
            rid = r['id']
            flac, wav = out / f'{rid}.flac', out / f'{rid}.wav'
            try:
                urllib.request.urlretrieve(r['audio'][0]['src'], flac)
                source_sha256 = hashlib.sha256(flac.read_bytes()).hexdigest()
                subprocess.run([args.ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', str(flac), '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le', str(wav)], check=True)
            except Exception as error:  # noqa: BLE001 - one bad row must not end the fetch
                print('skip', rid, error)
                continue
            finally:
                if flac.exists():
                    os.remove(flac)
            with wave.open(str(wav)) as w:
                ms = w.getnframes() / 16
            if ms > args.max_seconds * 1000:
                os.remove(wav)
                continue
            samples.append(dict(id=rid.replace('-', '_'), expected_text=r['text'], input_language='en', audio_path=str(wav), audio_sha256=hashlib.sha256(wav.read_bytes()).hexdigest(), source_audio_sha256=source_sha256, duration_ms=ms))
            print(rid, round(ms), flush=True)
            if len(samples) >= args.count:
                break
        if len(samples) >= args.count:
            break
    if len(samples) != args.count:
        raise RuntimeError('Requested corpus count not available; partial inputs retained, no complete manifest')
    manifest = dict(public_corpus_reuse_confirmed=True, provenance='openslr/librispeech_asr clean test split via the public Hugging Face datasets server; CC BY 4.0; held-out public English read speech, not accented dictation', samples=samples,
                    preparation=dict(ffmpeg_version=ffmpeg_version, codec='pcm_s16le', sample_rate=16000, channels=1),
                    selection=dict(count=args.count, offsets=args.offsets, max_seconds=args.max_seconds), dataset_responses=responses)
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=1))
    print('samples', len(samples), 'total_s', round(sum(s['duration_ms'] for s in samples) / 1000))


if __name__ == '__main__':
    main()
