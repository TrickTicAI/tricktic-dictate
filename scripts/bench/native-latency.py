#!/usr/bin/env python3
"""WSL runner for an owned Windows native dictation probe and public speech.
Records every failure and scores final durable text; private audio/results stay
outside source. Does not start/stop workers or touch the user's clipboard.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics
import subprocess
from urllib.parse import urlparse
import wave


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('probe', 'manifest', 'model', 'ffmpeg', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--url', required=True)
    parser.add_argument('--repeats', type=int, default=1)
    parser.add_argument('--trailing-silence-ms', type=int, default=0)
    args = parser.parse_args()
    url = urlparse(args.url)
    if url.scheme != 'http' or url.hostname not in ('localhost', '127.0.0.1', '::1') or url.path != '/inference' or url.username or url.password or url.query or url.fragment:
        parser.error('Use an owned loopback /inference endpoint')
    if args.repeats < 1 or args.trailing_silence_ms < 0:
        parser.error('Use positive repetitions and nonnegative silence')
    repo = Path(__file__).resolve().parents[2]
    if any(p.resolve().is_relative_to(repo) for p in (args.output, args.manifest)):
        parser.error('Keep speech, manifests and results outside source')
    manifest = json.loads(args.manifest.read_text(encoding='utf-8-sig'))
    if not manifest.get('public_corpus_reuse_confirmed') or not manifest.get('samples'):
        parser.error('Public-corpus provenance is required')
    spec = importlib.util.spec_from_file_location('scoring', Path(__file__).with_name('dictation-latency.py'))
    scoring = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(scoring)
    args.output.mkdir(parents=True, exist_ok=False)
    native = lambda p: subprocess.check_output(['wslpath', '-w', str(p.resolve())], text=True).strip()
    rows = []
    for repeat in range(args.repeats):
        for index, sample in enumerate(manifest['samples']):
            source = Path(sample['audio_path'])
            if source.resolve().is_relative_to(repo):
                parser.error('Audio must be outside source')
            digest = hashlib.sha256(source.read_bytes()).hexdigest()
            if sample.get('audio_sha256') and sample['audio_sha256'] != digest:
                raise ValueError('Source audio hash differs from the manifest')
            audio = source
            if args.trailing_silence_ms:
                audio = args.output / f'audio-{repeat}-{index}.wav'
                with wave.open(str(source), 'rb') as original:
                    if original.getsampwidth() != 2:
                        raise ValueError('PCM16 WAV required')
                    with wave.open(str(audio), 'wb') as out:
                        out.setparams(original.getparams())
                        out.writeframes(original.readframes(original.getnframes()))
                        out.writeframes(bytes(args.trailing_silence_ms * original.getframerate() // 1000 * original.getnchannels() * 2))
            output = args.output / f'trial-{repeat}-{index}'
            row = {'sample': sample['id'], 'repeat': repeat, 'audio_sha256': digest}
            try:
                run = subprocess.run([str(args.probe.resolve()), native(audio), native(output), args.url, native(args.model), native(args.ffmpeg)], capture_output=True, timeout=180)
                (args.output / f'trial-{repeat}-{index}.log').write_bytes(run.stdout + run.stderr)
                row['exit_code'] = run.returncode
                if run.returncode == 0:
                    row.update(json.loads((output / 'timing.json').read_text()))
                    jobs = list((output / 'Data/jobs').glob('*.json'))
                    job = json.loads(jobs[0].read_text())
                    text = job['status']['result']['transcript']['raw_text']
                    reference = scoring.normalized(sample['expected_text'])
                    row['word_edits'] = scoring.distance(reference, scoring.normalized(text))
                    row['reference_words'] = len(reference)
            except subprocess.TimeoutExpired:
                row['exit_code'] = 'timeout'
            rows.append(row)
            (args.output / 'results.json').write_text(json.dumps(rows, indent=2))
            print(json.dumps({k: row[k] for k in ('sample', 'repeat', 'exit_code', 'stop_to_transcript_ms', 'word_edits') if k in row}), flush=True)
    successes = [r for r in rows if r['exit_code'] == 0]
    times = [r['stop_to_transcript_ms'] for r in successes]
    summary = {'scope': 'Native replay/journal, preview, durable transcript; excludes WebView and insertion',
               'trials': len(rows), 'failures': len(rows) - len(successes), 'trailing_silence_ms': args.trailing_silence_ms,
               'probe_sha256': hashlib.sha256(args.probe.read_bytes()).hexdigest(),
               'median_ms': statistics.median(times) if times else None,
               'observed_p95_ms': scoring.percentile(times, .95) if times else None,
               'max_ms': max(times) if times else None,
               'word_edits': sum(r['word_edits'] for r in successes),
               'reference_words': sum(r['reference_words'] for r in successes)}
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary))
    if summary['failures']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
