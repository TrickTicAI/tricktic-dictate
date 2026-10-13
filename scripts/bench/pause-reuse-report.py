#!/usr/bin/env python3
"""Aggregate a same-build Windows pause replay without exporting speech or text.

Inputs stay private. Require paired source/derived audio identities and use the
versioned language scorer. This is saved public speech with a constructed pause,
not a physical microphone, insertion, cleanup or S25 measurement.
"""
import argparse
import hashlib
import importlib.util
import json
import math
from pathlib import Path


def percentile(values, fraction):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def report(fixture, baseline, candidate):
    scoring_path = Path(__file__).resolve().parents[2] / 'tools/evaluate-language/evaluate.py'
    spec = importlib.util.spec_from_file_location('language_scoring', scoring_path)
    scoring = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(scoring)
    samples = {s['id']: s for s in fixture['samples']}
    def index(rows, arm):
        indexed = {}
        for row in rows:
            key = (row['id'], row['repeat'])
            if key in indexed or row['baseline'] is not arm or row['id'] not in samples:
                raise ValueError('Duplicate, wrong-arm or unknown sample')
            for name in ('stop_to_raw_ms', 'first_preview_ms', 'extension_preview_ms'):
                value = row[name]
                if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                    raise ValueError('Invalid timing')
            if 'preview_inference_calls' not in row['stages']['details']:
                raise ValueError('Missing recognition trace; do not infer zero work')
            indexed[key] = row
        return indexed
    left, right = index(baseline, True), index(candidate, False)
    if not left or left.keys() != right.keys():
        raise ValueError('Require nonempty complete paired attempts')
    for key in left:
        for name in ('source_sha256', 'derived_pcm_sha256', 'warm'):
            if left[key][name] != right[key][name]:
                raise ValueError('Audio or warmth changed between paired attempts')
        expected_hash = samples[key[0]].get('audio_sha256')
        if expected_hash and left[key]['source_sha256'] != expected_hash:
            raise ValueError('Source does not match benchmark corpus')
    output = {'kind': 'windows_gpu_saved_speech_constructed_pause',
              'percentile_method': 'nearest_rank', 'paired_attempts': len(left),
              'scorer_sha256': hashlib.sha256(scoring_path.read_bytes()).hexdigest(),
              'arms': {}}
    for name, rows in [('baseline', baseline), ('candidate', candidate)]:
        cohorts = {}
        for warm in (False, True):
            group = [r for r in rows if r['warm'] is warm]
            cohorts['warm' if warm else 'cold_first_preview'] = {
                'n': len(group),
                **{stage: {'p50_ms': percentile([r[stage] for r in group], .5),
                           'p95_ms': percentile([r[stage] for r in group], .95)}
                   for stage in ('first_preview_ms', 'extension_preview_ms', 'stop_to_raw_ms')},
                'preview_inference_calls': sum(r['stages']['details'].get('preview_inference_calls', 0) for r in group),
                'final_inference_calls': sum(r['stages']['details'].get('final_inference_calls', 0) for r in group),
            }
        quality = {}
        for language in ('en', 'he', 'mixed'):
            group = [r for r in rows if samples[r['id']]['input_language'] == language]
            scores = [scoring.score(samples[r['id']]['expected_text'], r['raw_text'], samples[r['id']].get('critical_checks', [])) for r in group]
            words = sum(len(scoring.normalized(samples[r['id']]['expected_text']).split()) for r in group)
            characters = sum(len(scoring.normalized(samples[r['id']]['expected_text'])) for r in group)
            word_edits = sum(s['word_edits'] for s in scores)
            character_edits = sum(s['normalized_character_edits'] for s in scores)
            quality[language] = {'n': len(group), 'unique_clips': len({r['id'] for r in group}),
                                 'word_edits': word_edits, 'reference_words': words,
                                 'wer': word_edits / words if words else None,
                                 'character_edits': character_edits,
                                 'cer': character_edits / characters if characters else None,
                                 'critical_failures': sum(bool(s['critical_errors'] or s['missing_script_flags']) for s in scores)}
        output['arms'][name] = {'cohorts': cohorts, 'quality': quality}
    output['changed_final_outputs'] = sum(left[k]['raw_text'] != right[k]['raw_text'] for k in left)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('fixture', 'baseline', 'candidate', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    data = report(*(json.loads(p.read_text(encoding='utf-8-sig')) for p in (args.fixture, args.baseline, args.candidate)))
    with args.output.open('x') as target:
        json.dump(data, target, indent=2)
        target.write('\n')


if __name__ == '__main__':
    main()
