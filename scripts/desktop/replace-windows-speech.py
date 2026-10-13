#!/usr/bin/env python3
"""Assemble private-worker Whisper builds over a verified retained Windows package.
Replaces runtime/whisper/ (Vulkan root and cpu/), the Whisper source/build receipts
and the native SBOM notice, and drops the components Writer never runs on Windows
(PRUNED). Writes a NEW package and candidate lock for review; never changes the
approved lock, installed files or models. llama.cpp's server, FFmpeg and other
notices stay byte-identical.
"""
import argparse
import hashlib
import json
import re
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'inference'))
from neutral_paths import copy_text, neutralize_tree  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
MARKERS = [b'writer: private listener', b'X-Writer-Instance']
AUTH_KEY = 'examples/server/writer-auth.h'
# MW-SEC-26: NeMo is the legacy upstream prebuilt that Writer refuses on Windows
# and the only carrier of the Visual C++ runtime DLLs (W-04, W-12). Writer starts
# only llama-server, whose import chain needs none of llama.cpp's tool libraries
# or the RPC backend (W-03).
PRUNED = re.compile(r'runtime/nemo/.+|notices/nemo-(?:cpu|vulkan)/.+|runtime/text/(?:cpu|vulkan)/'
                    r'(?:llama-(?:batched-bench|bench|cli|completion|fit-params|perplexity|quantize)-impl|ggml-rpc)\.dll')
PROVENANCE = ('Windows package with private-worker Whisper; see docs/security/windows-release-gate.md (MW-SEC-26). '
    'The whisper family (runtime/whisper/ Vulkan and runtime/whisper/cpu/, including CPU dispatch variants) is rebuilt from '
    'whisper.cpp 371b5a7561823ab2bb32142d2751e35e7534727b staged with scripts/inference/stage-whisper.py (warmup, preview '
    'preemption, encoder checkpoints, UTF-8 manifest, runtime/whisper/writer-auth.h private-worker authentication and '
    'cpp-httplib 0.58.0 pinned by runtime/whisper/httplib-source.json with pre-authentication limits) and built with '
    'scripts/inference/build-windows.py, then assembled by scripts/desktop/replace-windows-speech.py; patch hashes are in '
    'notices/writer-whisper-source.txt and configure receipts in notices/whisper-{cpu,vulkan}-*. NeMo-Speech.cpp is not '
    'shipped on Windows (Parakeet is unavailable there). The text runtime (runtime/text/cpu and vulkan) is llama-server '
    'built from llama.cpp b11393 with miniaudio dc3b628 by scripts/inference/build-windows-text.py '
    '(scripts/inference/windows-text-source.json; receipts in notices/text-*). FFmpeg and its notices are byte-identical '
    'to the previous package. Never substitute latest upstream binaries. Model weights are excluded.')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(root, entries):
    expected = set()
    for item in entries:
        rel = Path(item['path'])
        if rel.is_absolute() or '..' in rel.parts or item['path'] in expected:
            raise ValueError('Invalid or duplicate lock path: ' + item['path'])
        expected.add(item['path'])
        path = root / rel
        if (not path.is_file() or any((root / Path(*rel.parts[:i])).is_symlink() for i in range(1, len(rel.parts) + 1))
                or path.stat().st_size != item['bytes'] or digest(path) != item['sha256']):
            raise ValueError('Package differs from lock: ' + item['path'])
    actual = {p.relative_to(root).as_posix() for name in ['runtime', 'notices'] for p in (root / name).rglob('*') if p.is_file()}
    if actual != expected:
        raise ValueError('Package has unexpected/missing files: ' + ', '.join(sorted(actual ^ expected)))


def pe_x64(path):
    data = path.read_bytes()
    if len(data) < 256 or data[:2] != b'MZ':
        return False
    pe = int.from_bytes(data[60:64], 'little')
    return data[pe:pe + 4] == b'PE\0\0' and int.from_bytes(data[pe + 4:pe + 6], 'little') == 0x8664


def build_files(build, source, backend):
    receipt = (source / 'writer-source.txt').read_text()
    evidence = json.loads((build / 'writer-build.json').read_text())
    if evidence['source'] != receipt or evidence['backend'] != backend:
        raise ValueError(f'{backend} build source/backend differs from the staged source')
    # The receipt must describe the staged bytes, and its auth header must be this app's.
    values = dict(line.split('=', 1) for line in receipt.splitlines())
    for name, value in values.items():
        if '/' in name and digest(source / name) != value:
            raise ValueError('Staged source differs from receipt: ' + name)
    if values.get(AUTH_KEY) != digest(ROOT / 'runtime/whisper/writer-auth.h'):
        raise ValueError('Whisper source receipt is not for this checkout\'s writer-auth.h')
    files = {f.name: f for f in sorted((build / 'bin').iterdir()) if f.is_file() and f.suffix in ('.exe', '.dll')}
    required = {'whisper-server.exe', 'whisper.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu-x64.dll'}
    if not required <= set(files) or ('ggml-vulkan.dll' in files) != (backend == 'vulkan'):
        raise ValueError(f'{backend} build is incomplete or has the wrong GPU backend')
    for name, file in files.items():
        if file.is_symlink() or not pe_x64(file):
            raise ValueError('Expected an x64 PE file: ' + name)
    server = files['whisper-server.exe'].read_bytes()
    if not all(marker in server for marker in MARKERS):
        raise ValueError(f'Legacy {backend} whisper-server.exe; rebuild with the private-worker protocol')
    return files


def text_files(build, backend, pin):
    """Source-built llama-server (scripts/inference/build-windows-text.py), MW-SEC-26."""
    evidence = json.loads((build / 'writer-build.json').read_text())
    if evidence['backend'] != backend or evidence['llama'] != pin['llama'] or evidence['miniaudio'] != pin['miniaudio']:
        raise ValueError(f'text {backend} build is not for the pinned llama.cpp/miniaudio sources')
    if digest(build / 'source/vendor/miniaudio/miniaudio.h') != pin['miniaudio']['sha256']:
        raise ValueError(f'text {backend} build staged a different miniaudio')
    files = {f.name: f for f in sorted((build / 'package').iterdir()) if f.is_file()}
    required = {'llama-server.exe', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu-x64.dll'}
    if set(files) != set(evidence['files']) or not required <= set(files) or ('ggml-vulkan.dll' in files) != (backend == 'vulkan'):
        raise ValueError(f'text {backend} build is incomplete or differs from its receipt')
    for name, file in files.items():
        if file.is_symlink() or not pe_x64(file) or digest(file) != evidence['files'][name] or PRUNED.fullmatch('runtime/text/cpu/' + name):
            raise ValueError('Unexpected text runtime file: ' + name)
    return files


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--base', type=Path, required=True, help='retained package (runtime/, notices/)')
    p.add_argument('--base-lock', type=Path, required=True)
    p.add_argument('--build-root', type=Path, help='holds whisper-source, whisper-cpu, whisper-vulkan; omit for text-only replacement')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--lock-output', type=Path, required=True)
    p.add_argument('--text-root', type=Path, help='holds text-cpu and text-vulkan source builds; replaces runtime/text/')
    p.add_argument('--miniaudio-license', type=Path, help='miniaudio LICENSE at the pinned revision (with --text-root)')
    a = p.parse_args()
    if not a.build_root and not a.text_root:
        p.error('Provide --build-root, --text-root, or both')
    text_pin = json.loads((ROOT / 'scripts/inference/windows-text-source.json').read_text())
    text = {}
    if a.text_root:
        text = {backend: text_files(a.text_root / f'text-{backend}', backend, text_pin) for backend in ['cpu', 'vulkan']}
        if not a.miniaudio_license or digest(a.miniaudio_license) != text_pin['miniaudio']['license_sha256']:
            p.error('--miniaudio-license must match the pinned miniaudio LICENSE')
    old = json.loads(a.base_lock.read_text())
    verify(a.base, old['files'])
    output = a.output.resolve()
    if output.is_relative_to(ROOT):
        p.error('Keep binary packages outside source')
    source = a.build_root / 'whisper-source' if a.build_root else None
    builds = {backend: build_files(a.build_root / f'whisper-{backend}', source, backend) for backend in ['vulkan', 'cpu']} if source else {}
    license_notices = [e['path'] for e in old['files'] if e['path'] in ('notices/whisper.cpp-LICENSE.txt', 'notices/whisper-cpu-LICENSE.txt')]
    for path in license_notices if source else []:
        if digest(a.base / path) != digest(source / 'LICENSE'):
            raise ValueError('Retained Whisper license differs from the staged source: ' + path)
    output.mkdir(parents=True, exist_ok=False)

    def copy(src, relative):
        dest = output / relative
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dest)

    replaced_text = re.compile(r'runtime/text/.+|notices/text-.+|notices/(?:llama|miniaudio)-LICENSE\.txt') if text else re.compile(r'(?!)')
    retained = [e for e in old['files'] if not (builds and e['path'].startswith('runtime/whisper/')) and not PRUNED.fullmatch(e['path'])
                and not replaced_text.fullmatch(e['path'])]
    for item in retained:
        copy(a.base / item['path'], item['path'])
    for backend, files in builds.items():
        for file in files.values():
            copy(file, 'runtime/whisper/' + ('cpu/' if backend == 'cpu' else '') + file.name)
        build = a.build_root / f'whisper-{backend}'
        # Receipts keep the configuration but not the builder's home folder (MW-RELEASE-14).
        copy_text(build / 'writer-build.json', output / f'notices/whisper-{backend}-build.json')
        copy_text(build / 'CMakeCache.txt', output / f'notices/whisper-{backend}-cmake.txt')
        compiler = list(build.glob('CMakeFiles/*/CMakeCXXCompiler.cmake'))
        if len(compiler) != 1:
            raise ValueError(f'Expected one CMake compiler identification receipt for {backend}')
        copy_text(compiler[0], output / f'notices/whisper-{backend}-compiler.cmake')
    for backend, files in text.items():
        build = a.text_root / f'text-{backend}'
        for file in files.values():
            copy(file, f'runtime/text/{backend}/{file.name}')
        copy_text(build / 'writer-build.json', output / f'notices/text-{backend}-build.json')
        copy_text(build / 'build/CMakeCache.txt', output / f'notices/text-{backend}-cmake.txt')
        copy_text(build / 'build-ggml/CMakeCache.txt', output / f'notices/text-{backend}-ggml-cmake.txt')
    if text:
        copy(a.text_root / 'text-cpu/source/LICENSE', 'notices/llama-LICENSE.txt')
        copy(a.miniaudio_license, 'notices/miniaudio-LICENSE.txt')
    if source:
        copy(source / 'writer-source.txt', 'notices/writer-whisper-source.txt')
    copy(ROOT / 'scripts/inference/native-runtime.cdx.json', 'notices/native-runtime.cdx.json')
    # Retained receipts from earlier builds lose their home folders too.
    replaced = {'notices/native-runtime.cdx.json', *('notices/' + p for p in neutralize_tree(output / 'notices'))}
    if source:
        replaced |= {'notices/writer-whisper-source.txt'} | {
            f'notices/whisper-{backend}-{kind}' for backend in ('cpu', 'vulkan')
            for kind in ('build.json', 'cmake.txt', 'compiler.cmake')}
    entries = []
    for item in retained:
        if item['path'] not in replaced:
            if digest(output / item['path']) != item['sha256']:
                raise ValueError('Retained input changed while copying: ' + item['path'])
            entries.append(item)
    for file in sorted(output.rglob('*')):
        rel = file.relative_to(output).as_posix()
        if file.is_file() and ((builds and rel.startswith('runtime/whisper/')) or rel in replaced or (text and replaced_text.fullmatch(rel))
                               or rel == 'notices/miniaudio-LICENSE.txt'):
            entries.append({'path': rel, 'sha256': digest(file), 'bytes': file.stat().st_size,
                            'role': 'runtime' if rel.startswith('runtime/') else 'notice'})
    entries.sort(key=lambda e: e['path'])
    verify(output, entries)
    lock = {**old, 'provenance': PROVENANCE, 'files': entries}
    a.lock_output.write_text(json.dumps(lock, indent=2) + '\n')
    print(f'Candidate package: {len(entries)} verified inputs; native worker smoke still required.')


if __name__ == '__main__':
    main()
