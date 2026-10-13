#!/usr/bin/env python3
"""Run real Whisper artifacts on a named backend. Audio/results stay outside source.
Example: qualify.py --runtime /path/whisper-cli --cache /private/qualification \
    --samples /private/samples.json --backend cpu --models whisper-tiny
Samples: [{"id":"public-en","input_language":"en","audio_path":"...",
           "expected_text":"optional reference"}]. No quality claims from exit code alone.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import unicodedata
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[2]

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(4 * 1024 * 1024), b''): h.update(block)
    return h.hexdigest()

def words(text):
    return re.findall(r'\w+', unicodedata.normalize('NFC', text).casefold(), flags=re.UNICODE)

def errors(reference, actual):
    a, b = words(reference), words(actual)
    last = list(range(len(b) + 1))
    for i, word in enumerate(a, 1):
        current = [i]
        for j, other in enumerate(b, 1): current.append(min(current[-1]+1, last[j]+1, last[j-1]+(word != other)))
        last = current
    return {'word_errors': last[-1], 'reference_words': len(a), 'wer': last[-1]/len(a) if a else None}

def fetch(model, cache, reuse):
    path = cache / model['file_name']
    for source in [path] + [directory / model['file_name'] for directory in reuse]:
        if source.is_file() and source.stat().st_size == model['bytes'] and digest(source) == model['sha256']:
            return source
    if path.exists(): raise ValueError('Existing cache artifact does not match; refusing to replace it')
    partial = path.with_suffix('.part')
    offset = partial.stat().st_size if partial.exists() else 0
    if offset > model['bytes']: raise ValueError('Partial file exceeds pinned size')
    with urlopen(Request(model['source_url'], headers={'Range': f'bytes={offset}-'} if offset else {}), timeout=60) as response:
        append = offset > 0 and response.status == 206
        if append and not response.headers.get('Content-Range', '').startswith(f'bytes {offset}-'):
            raise ValueError('Download resumed at an unexpected offset')
        with partial.open('ab' if append else 'wb') as output:
            shutil.copyfileobj(response, output, 4 * 1024 * 1024)
    if partial.stat().st_size != model['bytes'] or digest(partial) != model['sha256']:
        raise ValueError('Artifact verification failed')
    partial.rename(path)
    return path

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--runtime', type=Path, required=True)
    p.add_argument('--cache', type=Path, required=True)
    p.add_argument('--samples', type=Path, required=True)
    p.add_argument('--backend', choices=['cpu', 'vulkan'], required=True)
    p.add_argument('--models', default='')
    p.add_argument('--threads', type=int, default=4)
    p.add_argument('--reuse-dir', type=Path, action='append', default=[])
    args = p.parse_args()
    cache = args.cache.resolve()
    if cache.is_relative_to(ROOT): p.error('Qualification cache must be outside the repository')
    cache.mkdir(parents=True, exist_ok=True)
    windows = args.runtime.suffix.lower() == '.exe' and os.name != 'nt'
    def native(path):
        return subprocess.check_output(['wslpath', '-w', str(path.resolve())], text=True).strip() if windows else str(path.resolve())
    samples = json.loads(args.samples.read_text())
    models = json.loads((ROOT / 'apps/api/src/desktop_settings/model-catalog.json').read_text())
    models = [m for m in models if m.get('runtime') == 'whisper_cpp']
    wanted = set(args.models.split(',')) if args.models else {m['id'] for m in models}
    unknown = wanted - {m['id'] for m in models}
    if unknown: p.error('Unknown model IDs: ' + ', '.join(sorted(unknown)))
    runtime_hashes = {f.name: digest(f) for f in [args.runtime] + sorted(args.runtime.parent.glob('*.dll'))}
    receipt = {'backend': args.backend, 'runtime_files_sha256': runtime_hashes, 'threads': args.threads,
               'platform': 'windows' if windows or os.name == 'nt' else os.uname().sysname,
               'scope': 'Direct CLI qualification; not installed-app acceptance or general language quality', 'results': []}
    for model in models:
        if model['id'] not in wanted: continue
        try:
            path = fetch(model, cache, args.reuse_dir)
            for sample in samples:
                language = sample['input_language']
                if language not in model['input_languages']: continue
                name = model['id'] + '-' + sample['id'] + '-' + args.backend
                if not re.fullmatch(r'[a-zA-Z0-9-]+', name): raise ValueError('Sample IDs must be alphanumeric with hyphens')
                output = cache / name
                command = [str(args.runtime), '-m', native(path), '-f', native(Path(sample['audio_path'])), '-l', 'auto' if language == 'mixed' else language,
                           '-t', str(args.threads), '-nt', '-otxt', '-of', native(output)]
                if args.backend == 'cpu': command.append('-ng')
                start = time.monotonic()
                run = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1800)
                logs = run.stderr.decode('utf-8', errors='replace')
                output.with_suffix('.log').write_text(logs)
                text = output.with_suffix('.txt').read_text(encoding='utf-8').strip() if output.with_suffix('.txt').exists() else ''
                backend_proved = ('use gpu    = 0' in logs or 'use gpu = 0' in logs) if args.backend == 'cpu' else 'using Vulkan' in logs
                row = {'model': model['id'], 'sha256': model['sha256'], 'sample': sample['id'], 'language': language,
                       'audio_sha256': digest(Path(sample['audio_path'])), 'elapsed_ms': round((time.monotonic()-start)*1000),
                       'exit_code': run.returncode, 'backend_proved': backend_proved,
                       'transcribed': bool(text) and run.returncode == 0, 'text_sha256': hashlib.sha256(text.encode()).hexdigest()}
                if sample.get('expected_text'):
                    row.update(errors(sample['expected_text'], text))
                    row['scripts_preserved'] = all(not re.search(pattern, sample['expected_text']) or bool(re.search(pattern, text)) for pattern in [r'[\u0590-\u05ff]', '[a-zA-Z]'])
                receipt['results'].append(row)
                print(json.dumps({k:v for k,v in row.items() if k not in ['sha256', 'audio_sha256', 'text_sha256']}), flush=True)
                (cache / ('receipt-' + args.backend + '.json')).write_text(json.dumps(receipt, indent=2))
        except Exception as error:
            receipt['results'].append({'model': model['id'], 'failure': type(error).__name__})
            print(model['id'], type(error).__name__, flush=True)
            (cache / ('receipt-' + args.backend + '.json')).write_text(json.dumps(receipt, indent=2))
    return int(any(not row.get('transcribed') or not row.get('backend_proved') for row in receipt['results']))

if __name__ == '__main__': raise SystemExit(main())
