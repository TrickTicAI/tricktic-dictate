#!/usr/bin/env python3
"""Opt-in, sequential saved-speech latency/quality test against a local worker.

Inputs and raw output stay outside the repository. No model loading, microphone,
clipboard, cloud calls, or application preference changes. Windows curl permits
WSL to reach a Windows loopback-only worker without exposing a listening service.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import random
import re
import statistics
import subprocess
import time
import unicodedata
from urllib.parse import urlparse
import wave


def normalized(text):
    # Match tools/evaluate-language: retain letters/marks/numbers in both scripts;
    # punctuation separates words instead of joining neighboring tokens.
    text = unicodedata.normalize('NFKC', text).casefold()
    return ''.join(c if unicodedata.category(c)[0] in 'LMN' else ' ' for c in text).split()


def distance(a, b):
    row = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        nxt = [i]
        for j, y in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j - 1] + (x != y)))
        row = nxt
    return row[-1]


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(p * len(values)) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--url', required=True)
    parser.add_argument('--curl', default='curl')
    parser.add_argument('--windows-curl', action='store_true')
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--profiles', default='current,single-pass,short-context')
    args = parser.parse_args()
    u = urlparse(args.url)
    if u.scheme != 'http' or u.hostname not in ('127.0.0.1', 'localhost', '::1') or u.username or u.password or u.query or u.fragment or u.path != '/inference':
        parser.error('Use a loopback HTTP /inference endpoint without credentials/query')
    repo = Path(__file__).resolve().parents[2]
    manifest_path, output = args.manifest.resolve(), args.output.resolve()
    if any(p.is_relative_to(repo) for p in (manifest_path, output)):
        parser.error('Keep manifests, speech and raw results outside source')
    manifest = json.loads(manifest_path.read_text())
    if not manifest.get('public_corpus_reuse_confirmed') or not manifest.get('samples') or args.repeats < 1:
        parser.error('Supply existing public-corpus provenance and positive repeat count')
    output.mkdir(parents=True, exist_ok=False)

    def native(path):
        return subprocess.check_output(['wslpath', '-w', str(path)], text=True).strip() if args.windows_curl else str(path)

    samples = []
    for entry in manifest['samples']:
        path = Path(entry['audio_path']).resolve()
        if path.is_relative_to(repo) or not re.fullmatch(r'[\w-]+', entry['id']):
            parser.error('Keep speech outside source and use simple sample ids')
        content = path.read_bytes()
        digest = hashlib.sha256(content).hexdigest()
        if entry.get('audio_sha256') and digest != entry['audio_sha256']:
            raise ValueError('Audio hash mismatch')
        with wave.open(str(path), 'rb') as audio:
            if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (16000, 1, 2):
                raise ValueError('Supply 16 kHz mono signed 16-bit WAV, normalized before timing')
            duration_ms = audio.getnframes() / 16
        samples.append({**entry, 'audio_path': str(path), 'audio_sha256': digest, 'duration_ms': duration_ms})

    # Each request resets every experimental parameter; server request settings
    # must never accidentally bleed from one configuration into the next.
    profiles = {
        'current': {'best_of': 2, 'beam_size': -1, 'temperature_inc': .2, 'audio_ctx': 0},
        'single-pass': {'best_of': 1, 'beam_size': 1, 'temperature_inc': 0, 'audio_ctx': 0},
        'short-context': {'best_of': 1, 'beam_size': 1, 'temperature_inc': 0, 'audio_ctx': 'adaptive'},
    }
    for context in (512, 768):
        profiles[f'context-{context}'] = {**profiles['current'], 'audio_ctx': context}
        profiles[f'context-{context}-no-timestamps'] = {**profiles[f'context-{context}'], 'no_timestamps': 'true'}
    profiles['no-timestamps'] = {**profiles['current'], 'no_timestamps': 'true'}
    for context in (512, 768):
        capped = profiles[f'context-{context}-no-timestamps']
        # Whisper retries a low-confidence window at rising temperatures; a hard clip
        # can cost five extra decodes. These bound or remove that fallback.
        profiles[f'context-{context}-no-timestamps-fallback-0.5'] = {**capped, 'temperature_inc': .5}
        profiles[f'context-{context}-no-timestamps-single-pass'] = {**capped, 'best_of': 1, 'beam_size': 1, 'temperature_inc': 0}
    selected = args.profiles.split(',')
    if any(name not in profiles for name in selected):
        parser.error('Unknown profile')
    profiles = {name: profiles[name] for name in set(selected) | {'current'}}
    runs = []

    def request(sample, profile, repeat):
        parameters = dict(profiles[profile])
        if parameters['audio_ctx'] == 'adaptive':
            # 50 encoder positions/second, at least 5.12 s including headroom.
            parameters['audio_ctx'] = min(1500, max(256, math.ceil((sample['duration_ms'] / 1000 + .5) * 50 / 64) * 64))
        parameters.update(no_timestamps=parameters.get('no_timestamps', 'false'), language=sample['input_language'], translate='false', response_format='json', temperature=0, prompt='')
        response = output / f'{len(runs):03d}-response.json'
        command = [args.curl, '--silent', '--show-error', '--fail-with-body', '--noproxy', '*', '--max-time', '90', '--output', native(response), '--write-out', '%{time_total}', '--form', 'file=@' + native(Path(sample['audio_path']))]
        for key, value in parameters.items():
            command += ['--form-string', f'{key}={value}']
        started = time.perf_counter()
        result = subprocess.run(command + [args.url], text=True, capture_output=True, timeout=100)
        wall_ms = (time.perf_counter() - started) * 1000
        if result.returncode:
            raise RuntimeError(f'Inference failed (curl exit {result.returncode}); inspect private response {response}')
        payload = json.loads(response.read_text(encoding='utf-8-sig'))
        raw = payload.get('text')
        if not isinstance(raw, str):
            raise ValueError('Worker response missing transcript')
        reference = normalized(sample['expected_text'])
        row = dict(sample=sample['id'], duration_ms=sample['duration_ms'], profile=profile, repeat=repeat,
                   worker_http_ms=float(result.stdout) * 1000, client_wall_ms=wall_ms,
                   reference_words=len(reference), word_edits=distance(reference, normalized(raw)),
                   raw_text=raw, raw_sha256=hashlib.sha256(raw.encode()).hexdigest(), parameters=parameters)
        runs.append(row)
        (output / 'runs.json').write_text(json.dumps({'manifest_sha256': hashlib.sha256(manifest_path.read_bytes()).hexdigest(), 'runs': runs}, ensure_ascii=False, indent=2))
        print(json.dumps({k: row[k] for k in ('sample', 'duration_ms', 'profile', 'repeat', 'worker_http_ms', 'word_edits')}), flush=True)

    # Report the first request separately; don't count it as warmed performance.
    request(samples[0], 'current', -1)
    cases = [(sample, profile, repeat) for repeat in range(args.repeats) for sample in samples for profile in selected]
    random.Random(20260908).shuffle(cases)
    for sample, profile, repeat in cases:
        request(sample, profile, repeat)
    summary = []
    for profile in selected:
        rows = [r for r in runs if r['profile'] == profile and r['repeat'] >= 0]
        values = [r['worker_http_ms'] for r in rows]
        summary.append(dict(profile=profile, trials=len(rows), median_ms=statistics.median(values), p95_ms=percentile(values, .95), max_ms=max(values), under_1000=sum(v < 1000 for v in values), under_2000=sum(v < 2000 for v in values), word_edits=sum(r['word_edits'] for r in rows), reference_words=sum(r['reference_words'] for r in rows), per_sample=[dict(sample=s['id'], duration_ms=s['duration_ms'], median_ms=statistics.median(r['worker_http_ms'] for r in rows if r['sample'] == s['id'])) for s in samples]))
    for sample in samples:
        if hashlib.sha256(Path(sample['audio_path']).read_bytes()).hexdigest() != sample['audio_sha256']:
            raise ValueError('Source audio changed during inference')
    receipt = dict(scope='Sequential warm saved-WAV worker HTTP latency; excludes capture, Writer queue, persistence and paste. Small public Hebrew corpus, not broad quality acceptance.', first_request_ms=runs[0]['worker_http_ms'], manifest_sha256=hashlib.sha256(manifest_path.read_bytes()).hexdigest(), summary=summary)
    (output / 'summary.json').write_text(json.dumps(receipt, indent=2))
    print(json.dumps(receipt), flush=True)


if __name__ == '__main__':
    main()
