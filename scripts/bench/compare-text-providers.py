#!/usr/bin/env python3
"""Compare complete Super responses through an isolated Writer API.

The API owns provider keys. Input is an explicitly selected private manifest:
{"samples":[{"id":"short", "text":"...", "profile":{...}, "context":{...}}]}.
Optional job_id uses a saved job in that isolated API, including its vocabulary.
No settings or history are written by the requests; output contains private text.
"""
import argparse
import copy
import json
import math
import statistics
import time
import urllib.error
import urllib.request
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--samples', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--repeats', type=int, choices=range(1, 6), default=3)
    parser.add_argument('--candidates', default='gemini:gemini-3.8-flash,grok:grok-4.6',
        help='Comma-separated provider:model pairs, e.g. openai:gpt-5.6-luna,grok:grok-4.3. '
             'OpenRouter slugs keep their slash: openrouter:anthropic/claude-haiku-4.5')
    args = parser.parse_args()
    samples = json.loads(args.samples.read_text(encoding='utf-8'))['samples']
    if not samples or len(samples) > 20 or args.output.exists():
        parser.error('Use 1–20 explicit samples and a fresh output file.')
    candidates = [tuple(item.split(':', 1)) for item in args.candidates.split(',')]
    if len(candidates) < 2 or any(len(c) != 2 or not all(c) for c in candidates):
        parser.error('Give at least two provider:model candidates.')
    rows = []
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for repeat in range(args.repeats):
        for index, sample in enumerate(samples):
            # Alternate both by sample and repetition to reduce order/warm bias.
            order = candidates if (repeat + index) % 2 == 0 else candidates[::-1]
            for provider, model in order:
                profile = copy.deepcopy(sample['profile'])
                profile.update(text_model={'provider': provider, 'model': model}, fast_processing=True)
                data = {'profile': profile, 'text': sample['text'], 'context': sample.get('context', {})}
                if sample.get('job_id'):
                    data['job_id'] = sample['job_id']
                req = urllib.request.Request(args.url.rstrip('/') + '/api/v1/modes/preview',
                    data=json.dumps({'api_version': '1', 'data': data}).encode(),
                    headers={'Content-Type': 'application/json'})
                row = {'sample': sample['id'], 'repeat': repeat, 'provider': provider, 'model': model, 'fast': True}
                start = time.perf_counter()
                try:
                    with urllib.request.urlopen(req, timeout=130) as response:
                        row.update(json.load(response)['data'])
                    row['state'] = 'succeeded'
                except urllib.error.HTTPError as error:
                    # Writer returns sanitized errors, never raw provider bodies.
                    row.update(state='failed', http_status=error.code, error=json.load(error))
                except (urllib.error.URLError, TimeoutError):
                    row.update(state='failed', error='Transport or timeout failure')
                row['client_ms'] = round((time.perf_counter() - start) * 1000)
                rows.append(row)
                args.output.write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding='utf-8')
                print(json.dumps({k: row[k] for k in ['sample', 'repeat', 'provider', 'model', 'state', 'client_ms']}), flush=True)
    for provider, model in candidates:
        matched = [r for r in rows if r['provider'] == provider and r['model'] == model]
        times = sorted(r['diagnostics']['processing_ms'] for r in matched if r['state'] == 'succeeded')
        print(json.dumps({'provider': provider, 'model': model, 'successes': len(times), 'failures': len(matched)-len(times),
            'median_ms': statistics.median(times) if times else None,
            'observed_p95_ms': times[math.ceil(.95*len(times))-1] if times else None,
            'min_ms': min(times) if times else None, 'max_ms': max(times) if times else None}), flush=True)
    if any(r['state'] != 'succeeded' for r in rows):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
