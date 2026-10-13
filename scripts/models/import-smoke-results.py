#!/usr/bin/env python3
"""Import transcript-free reference measurements, bound to exact catalog hashes."""
import argparse
from datetime import date
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--cpu', type=Path, required=True)
p.add_argument('--vulkan', type=Path, required=True)
p.add_argument('--host', required=True)
p.add_argument('--date', required=True)
args = p.parse_args()
date.fromisoformat(args.date)
models = {m['id']: m for m in json.loads((ROOT/'apps/api/src/desktop_settings/model-catalog.json').read_text())}
summary = {identity: value for identity, value in json.loads((ROOT/'apps/api/src/desktop_settings/model-smoke-tests.json').read_text()).items() if models[identity]['runtime'] != 'whisper_cpp'}
models = {identity: model for identity, model in models.items() if model['runtime'] == 'whisper_cpp'}
for backend, path in [('cpu', args.cpu), ('vulkan', args.vulkan)]:
    raw = path.read_bytes()
    document = json.loads(raw)
    if document['backend'] != backend or document['platform'] != 'windows':
        p.error('Expected a matching Windows backend receipt')
    for result in document['results']:
        model = models.get(result['model'])
        if not model or result.get('sha256') != model['sha256'] or not result.get('transcribed') or not result.get('backend_proved'):
            p.error('Receipt contains an unverified, failed or changed artifact')
        if result['language'] not in model['input_languages']:
            p.error('Receipt uses an unsupported input language')
        record = summary.setdefault(model['id'], {'model_sha256': model['sha256'], 'host': args.host, 'date': args.date,
            'scope': 'Small public samples; not a general accuracy benchmark', 'tests': []})
        record['tests'].append({key: result[key] for key in ['sample','language','elapsed_ms','word_errors','reference_words']} |
                              {'backend': backend, 'receipt_sha256': hashlib.sha256(raw).hexdigest()})
for identity, model in models.items():
    tests = summary.get(identity, {}).get('tests', [])
    if any(not any(t['language'] == language and t['backend'] == backend for t in tests)
           for language in model['input_languages'] if language != 'mixed' for backend in ['cpu', 'vulkan']):
        p.error('Every catalog artifact needs CPU and Vulkan samples for its English/Hebrew inputs')
(ROOT/'apps/api/src/desktop_settings/model-smoke-tests.json').write_text(json.dumps(summary, indent=2)+'\n')
print(f'Imported reference measurements for {len(summary)} artifacts; mixed speech remains separately unqualified')
