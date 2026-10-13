#!/usr/bin/env python3
"""Assemble protocol-compatible speech workers over a verified retained Mac package.
Writes a NEW package and candidate lock for review; never changes installed files.
"""
import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'inference'))
from neutral_paths import copy_text, neutralize_tree  # noqa: E402

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--base', type=Path, required=True)
p.add_argument('--base-lock', type=Path, required=True)
p.add_argument('--build-root', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--lock-output', type=Path, required=True)
a = p.parse_args()
root = Path(__file__).resolve().parents[2]
hash_file = lambda f: hashlib.sha256(f.read_bytes()).hexdigest()
lock = json.loads(a.base_lock.read_text())
for item in lock['files']:
    f = a.base / item['path']
    if f.stat().st_size != item['bytes'] or hash_file(f) != item['sha256']:
        raise ValueError('Retained base differs from lock: ' + item['path'])
if a.output.exists():
    p.error('Use a fresh output directory')
shutil.copytree(a.base, a.output)
for family in ['whisper', 'nemo']:
    shutil.rmtree(a.output / 'runtime' / family)
    shutil.copy2(a.build_root / ('whisper-source' if family == 'whisper' else 'nemo-auth-source') / 'writer-source.txt', a.output / 'notices' / f'writer-{family}-source.txt')
for backend in ['metal', 'cpu']:
    dest = a.output / 'runtime/whisper' / ('cpu' if backend == 'cpu' else '')
    shutil.copytree(a.build_root / f'whisper-{backend}/package', dest, dirs_exist_ok=True)
    nemo = a.build_root / f'nemo-{backend}/package'
    ndest = a.output / 'runtime/nemo' / backend
    shutil.copytree(nemo / 'bin', ndest / 'bin')
    (ndest / 'lib').mkdir()
    for f in (nemo / 'lib').glob('*.dylib'):
        shutil.copy2(f, ndest / 'lib' / f.name)
    shutil.copy2(dest / 'writer-worker-host', ndest / 'bin/writer-worker-host')
    notices = a.output / 'notices' / f'nemo-{backend}'
    if notices.exists():
        shutil.rmtree(notices)
    shutil.copytree(nemo / 'share/licenses', notices)
    for family in ['whisper', 'nemo']:
        copy_text(a.build_root / f'{family}-{backend}/writer-build.json', a.output / 'notices' / f'{family}-{backend}-build.json')
sp = a.build_root / 'sentencepiece-source'
for name in ['LICENSE', 'third_party/absl/LICENSE', 'third_party/darts_clone/LICENSE', 'third_party/protobuf-lite/LICENSE']:
    dest = a.output / 'notices/sentencepiece' / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(sp / name, dest)
# Retained receipts from earlier builds lose their home folders too (MW-RELEASE-14).
neutralize_tree(a.output / 'notices')
entries = []
for f in sorted(a.output.rglob('*')):
    if not f.is_file():
        continue
    rel = f.relative_to(a.output).as_posix()
    runtime = rel.startswith('runtime/')
    if runtime:
        header = subprocess.check_output(['file', '-b', str(f)], text=True)
        if 'Mach-O' not in header or 'arm64' not in header:
            raise ValueError('Expected arm64 Mach-O: ' + rel)
        for line in subprocess.check_output(['otool', '-L', str(f)], text=True).splitlines()[1:]:
            dep = line.strip().split(' (')[0]
            if not dep.startswith(('@rpath/', '@loader_path/', '/usr/lib/', '/System/Library/')):
                raise ValueError('Nonportable dependency: ' + dep)
    entries.append({'path': rel, 'bytes': f.stat().st_size, 'sha256': hash_file(f),
                    'role': 'runtime' if runtime else 'notice',
                    **({'executable': True} if runtime and f.suffix != '.dylib' else {})})
result = {**lock, 'provenance': lock['provenance'] + ' Speech workers rebuilt from pinned sources with private-worker authentication; NeMo links pinned static SentencePiece. See source/build receipts in notices.', 'files': entries}
a.lock_output.write_text(json.dumps(result, indent=2) + '\n')
print('Candidate package:', len(entries), 'verified inputs; native speech acceptance still required.')
