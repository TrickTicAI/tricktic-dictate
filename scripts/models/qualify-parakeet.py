#!/usr/bin/env python3
"""Verify pinned NeMo Windows packages and run Parakeet on explicit public/test audio.
Direct-runtime qualification only; this does not register a Writer provider.
Audio, transcripts, model weights and runtime packages remain outside source.
"""
import argparse
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import time
import zipfile
from qualify import ROOT, digest, errors, fetch


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--cache', type=Path, required=True)
    p.add_argument('--audio', type=Path, required=True)
    p.add_argument('--reference', type=Path)
    p.add_argument('--backend', choices=['cpu', 'vulkan'], required=True)
    args = p.parse_args()
    cache = args.cache.resolve()
    if cache.is_relative_to(ROOT): p.error('Use a private cache outside source')
    cache.mkdir(parents=True, exist_ok=True)
    spec = json.loads(Path(__file__).with_name('parakeet.json').read_text())
    archive = fetch(spec['runtimes'][args.backend], cache, [])
    runtime = cache/args.backend
    with zipfile.ZipFile(archive) as z:
        for item in z.infolist():
            path = PurePosixPath(item.filename.replace('\\', '/'))
            if path.is_absolute() or '..' in path.parts or ':' in str(path) or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError('Archive contains an unsafe path or symbolic link')
            destination = runtime/Path(*path.parts)
            if item.filename.endswith(('/', '\\')):
                destination.mkdir(parents=True, exist_ok=True)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                with z.open(item) as source, destination.open('wb') as output:
                    shutil.copyfileobj(source, output)
    model = fetch(spec['model'], cache, [])
    executable = runtime/'bin/nemo-speech.exe'
    def native(path):
        return str(path.resolve()) if os.name == 'nt' else subprocess.check_output(['wslpath', '-w', str(path.resolve())], text=True).strip()
    result_file = cache/(args.backend+'-transcript-'+str(time.time_ns())+'.json')
    started = time.monotonic()
    result = subprocess.run([str(executable), 'transcribe', native(args.audio), '--model', native(model), '--backend', args.backend,
                             '--format', 'json', '--output', native(result_file), '--verbatim'], capture_output=True, text=True, timeout=300)
    elapsed = round((time.monotonic()-started)*1000)
    result_file.with_suffix('.stdout.log').write_text(result.stdout)
    result_file.with_suffix('.stderr.log').write_text(result.stderr)
    if result.returncode != 0: raise RuntimeError('Parakeet execution failed; inspect the private logs')
    output = json.loads(result_file.read_text(encoding='utf-8-sig'))
    transcript = output.get('text', '')
    receipt = {'model': spec['model']['id'], 'model_sha256': digest(model), 'runtime_archive_sha256': digest(archive),
               'backend_requested': args.backend, 'backend_proved': bool(re.search(r'backend=CPU\b' if args.backend == 'cpu' else r'backend=Vulkan\d*\b', result.stderr)),
               'audio_sha256': digest(args.audio), 'elapsed_ms': elapsed, 'transcribed': bool(transcript.strip()),
               'scope': 'One direct-runtime sample; not a Writer adapter or general language qualification'}
    if args.reference: receipt.update(errors(args.reference.read_text(), transcript))
    result_file.with_suffix('.receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(json.dumps(receipt))
    return int(not receipt['backend_proved'] or not receipt['transcribed'])

if __name__ == '__main__': raise SystemExit(main())
