#!/usr/bin/env python3
"""Content-free stage report from Writer latency exports; no session/audio input."""
import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import re
import uuid

STAGES = ('capture_readiness', 'preparation', 'upload', 'queue', 'asr', 'cleanup', 'raw_availability', 'insertion', 'stop_to_insertion', 'cloud_request')
FIELDS = {'recording_id', 'attempt_id', 'retry_of', 'provider_attempt', 'platform', 'provider', 'model_sha256', 'mode', 'warm', 'measurement', 'outcome', 'stages_ms', 'stage_outcomes'}
ENUMS = dict(platform={'windows', 'macos', 'linux', 'android', 'browser'}, provider={'local', 'openai', 'gemini', 'grok', 'groq', 'openrouter'}, mode={'regular', 'super', 'command'}, warm={'cold', 'warm', 'unknown'}, measurement={'unclassified', 'synthetic', 'physical_device'}, outcome={'pending', 'succeeded', 'failed'})

def validate(row):
    if not isinstance(row, dict) or set(row) != FIELDS:
        raise ValueError('Input is not a content-free timing record')
    for field, choices in ENUMS.items():
        if row[field] not in choices:
            raise ValueError('Invalid timing enumeration')
    for key in ('recording_id', 'attempt_id', 'retry_of'):
        if key == 'retry_of' and row[key] is None:
            continue
        if not isinstance(row[key], str) or str(uuid.UUID(row[key])) != row[key]:
            raise ValueError('Invalid attempt linkage')
    if not isinstance(row['model_sha256'], str) or not re.fullmatch('[a-f0-9]{64}', row['model_sha256']):
        raise ValueError('Invalid model identifier')
    if type(row['provider_attempt']) is not int or not 0 <= row['provider_attempt'] <= 1000000:
        raise ValueError('Invalid provider attempt')
    for key in ('stages_ms', 'stage_outcomes'):
        if not isinstance(row[key], dict) or not set(row[key]).issubset(STAGES):
            raise ValueError('Invalid stage map')
    if any(type(v) is not int or not 0 <= v <= 604800000 for v in row['stages_ms'].values()):
        raise ValueError('Invalid monotonic duration')
    if any(v not in ENUMS['outcome'] for v in row['stage_outcomes'].values()):
        raise ValueError('Invalid stage outcome')
    return row

def percentile(values, q):
    """Nearest-rank percentile; absent is null, zero is a genuine measurement."""
    if not values:
        return None
    return sorted(values)[max(0, math.ceil(len(values) * q)-1)]

def report(rows, provenance=None):
    deduplicated = {}
    for row in rows:
        validate(row)
        deduplicated[(row['platform'], row['attempt_id'], row['provider_attempt'])] = row
    groups = defaultdict(list)
    for row in deduplicated.values():
        groups[tuple(row[k] for k in ('measurement', 'platform', 'provider', 'model_sha256', 'mode', 'warm'))].append(row)
    result = []
    for identity, samples in sorted(groups.items()):
        group = dict(zip(('measurement', 'platform', 'provider', 'model_sha256', 'mode', 'warm'), identity))
        group.update(samples=len(samples), failures=sum(r['outcome']=='failed' for r in samples), pending=sum(r['outcome']=='pending' for r in samples), retries=sum(r['provider_attempt'] > 1 or r['retry_of'] is not None for r in samples))
        stages = {}
        for stage in STAGES:
            measured = [r for r in samples if stage in r['stages_ms']]
            success = [r['stages_ms'][stage] for r in measured if r['outcome']=='succeeded' and r['stage_outcomes'].get(stage, 'succeeded')=='succeeded']
            stages[stage] = dict(measured=len(measured), absent=len(samples)-len(measured), failures=sum(r['stage_outcomes'].get(stage, r['outcome'])=='failed' for r in measured), successful_samples=len(success), p50_ms=percentile(success,.5), p95_ms=percentile(success,.95))
        group['stages'] = stages
        result.append(group)
    return dict(percentile_method='nearest_rank; successful attempts/stages only', samples=len(deduplicated), superseded_rows=len(rows)-len(deduplicated), provenance=provenance, groups=result)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('exports', type=Path, nargs='+')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--provenance', type=Path, help='Noncontent receipt: build_commit, artifact_sha256, device, model, input, sample_count')
    parser.add_argument('--measurement', choices=ENUMS['measurement'], help='Explicit benchmark classification; physical_device requires provenance')
    args = parser.parse_args()
    rows = []
    try:
        for path in args.exports:
            if path.stat().st_size > 2 * 1024 * 1024:
                raise ValueError('Input exceeds bounded export size')
            value = json.loads(path.read_text())
            if set(value) == {'api_version', 'data'} and value['api_version']=='1':
                value = value['data']
            if not isinstance(value, dict) or set(value) != {'records'} or not isinstance(value['records'], list) or len(value['records']) > 256:
                raise ValueError('Expected a bounded Writer latency export')
            rows.extend(validate(r) for r in value['records'])
        provenance = json.loads(args.provenance.read_text()) if args.provenance else None
        if provenance is not None:
            if set(provenance) != {'build_commit', 'artifact_sha256', 'device', 'model', 'input', 'sample_count'} or not re.fullmatch('[a-f0-9]{40}', provenance['build_commit']) or not re.fullmatch('[a-f0-9]{64}', provenance['artifact_sha256']) or provenance['input'] not in {'saved_audio', 'microphone', 'synthetic_fixture'} or any(not isinstance(provenance[k], str) or len(provenance[k]) > 120 or '\n' in provenance[k] for k in ('device', 'model')):
                raise ValueError('Invalid noncontent provenance receipt')
            if provenance['sample_count'] != len({(r['platform'],r['attempt_id'],r['provider_attempt']) for r in rows}):
                raise ValueError('Provenance sample count does not match exported attempts')
        if (args.measurement == 'physical_device' or any(r['measurement']=='physical_device' for r in rows)) and (not provenance or provenance['input']=='synthetic_fixture'):
            raise ValueError('Physical-device classification requires build/device/model and real-audio provenance')
        if args.measurement:
            for row in rows:
                row['measurement'] = args.measurement
        result = json.dumps(report(rows, provenance), indent=2) + '\n'
    except (ValueError, KeyError, TypeError, OSError):
        parser.error('Rejected invalid timing export or provenance; no input content was printed')
    if args.output:
        args.output.write_text(result)
    else:
        print(result, end='')

if __name__ == '__main__':
    main()
