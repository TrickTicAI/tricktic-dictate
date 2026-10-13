#!/usr/bin/env python3
"""Compare retained and candidate native workers on explicitly supplied public fixtures.

Run with native Python on Windows/macOS. Manifest: model, backend (vulkan/metal/cpu),
baseline, candidate, cases [{id, audio, language}], public_corpus_reuse_confirmed.
Audio, model and transcripts stay outside source; the receipt contains hashes only.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import time
import urllib.request
import uuid


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--manifest', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    manifest = json.loads(args.manifest.read_text(encoding='utf-8'))
    if manifest.get('public_corpus_reuse_confirmed') is not True:
        p.error('Use explicitly identified public fixtures')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    model = Path(manifest['model'])
    inputs = {case['id']: digest(case['audio']) for case in manifest['cases']}
    result = {'backend': manifest['backend'], 'model_sha256': digest(model),
              'audio_sha256': inputs, 'runs': []}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    transcripts = {}
    for variant in ['baseline', 'candidate']:
        binary = Path(manifest[variant])
        # Private workers (writer-auth.h) take a per-launch capability and instance on
        # stdin, pick their own loopback port and require both on every request.
        capability, instance = secrets.token_hex(32), secrets.token_hex(32)
        command = [str(binary), '-m', str(model), '-l', 'auto', '-t',
                   '4' if manifest['backend'] == 'cpu' else '1', '-nt', '-ac', '768']
        if manifest['backend'] == 'cpu': command.append('-ng')
        log_path = output/(variant+'.log')
        start = time.monotonic()
        with log_path.open('wb') as log:
            child = subprocess.Popen(command, cwd=binary.parent, stdin=subprocess.PIPE,
                                     stdout=log, stderr=log,
                                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
            child.stdin.write((capability+'\n'+instance+'\n').encode()); child.stdin.close()
            headers = {'Authorization': 'Bearer '+capability}
            address = None
            try:
                while time.monotonic()-start < 180:
                    if child.poll() is not None: raise RuntimeError(variant+' exited before ready')
                    if address is None:
                        found = re.search(r'writer: private listener (\d+)',
                                          log_path.read_text(encoding='utf-8', errors='replace'))
                        if not found: time.sleep(.1); continue
                        address = 'http://127.0.0.1:'+found.group(1)
                    try:
                        with opener.open(urllib.request.Request(address+'/health', headers=headers), timeout=1) as response:
                            if response.headers.get('X-Writer-Instance') != instance:
                                raise RuntimeError(variant+' answered with the wrong instance')
                            health = json.load(response)
                        if health.get('warm') is True and health.get('writer_protocol') == 1: break
                    except (OSError, ValueError): pass
                    time.sleep(.1)
                else: raise RuntimeError(variant+' warmup timed out')
                run = {'variant': variant, 'binary_sha256': digest(binary),
                       'ready_seconds': round(time.monotonic()-start, 4), 'cases': []}
                text = log_path.read_text(encoding='utf-8', errors='replace')
                marker = {'vulkan': 'using Vulkan', 'metal': 'using MTL', 'cpu': 'use gpu    = 0'}[manifest['backend']]
                if marker not in text: raise RuntimeError('Requested backend not confirmed in '+variant)
                transcripts[variant] = {}
                for case in manifest['cases']:
                    boundary = uuid.uuid4().hex
                    chunks = []
                    for key, value in {'language': case['language'], 'response_format': 'json', 'temperature': '0'}.items():
                        chunks.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
                    chunks += [f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="sample.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode(),
                               Path(case['audio']).read_bytes(), f'\r\n--{boundary}--\r\n'.encode()]
                    start_case = time.monotonic()
                    request = urllib.request.Request(address+'/inference', data=b''.join(chunks),
                        headers={**headers, 'Content-Type': 'multipart/form-data; boundary='+boundary})
                    with opener.open(request, timeout=180) as response: data = json.load(response)
                    elapsed = time.monotonic()-start_case
                    transcript = data['text'].strip()
                    if not transcript: raise RuntimeError('Empty transcript for '+case['id'])
                    transcripts[variant][case['id']] = transcript
                    # Raw outputs remain in the private test directory for diagnosis.
                    (output/(variant+'-'+case['id']+'.json')).write_text(json.dumps(data, ensure_ascii=False), encoding='utf-8')
                    run['cases'].append({'id': case['id'], 'language': case['language'],
                        'seconds': round(elapsed, 4), 'text_sha256': hashlib.sha256(transcript.encode()).hexdigest()})
                result['runs'].append(run)
            finally:
                child.terminate()
                try: child.wait(timeout=10)
                except subprocess.TimeoutExpired: child.kill(); child.wait()
    comparisons = []
    for case in manifest['cases']:
        name = case['id']; before = transcripts['baseline'][name]; after = transcripts['candidate'][name]
        ratio = difflib.SequenceMatcher(None, before, after).ratio()
        comparisons.append({'id': name, 'identical_text': before == after, 'similarity': round(ratio, 4)})
    result['comparisons'] = comparisons
    result['inputs_unchanged'] = inputs == {case['id']: digest(case['audio']) for case in manifest['cases']}
    (output/'receipt.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8', newline='\n')
    if not result['inputs_unchanged'] or any(item['similarity'] < .95 for item in comparisons):
        raise RuntimeError('Regression needs review; see private outputs and receipt')
    print('Native baseline/candidate speech comparison passed; see hash-only receipt')


if __name__ == '__main__': main()
