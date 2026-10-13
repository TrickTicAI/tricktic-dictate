#!/usr/bin/env python3
"""Replace the complete text runtime in a verified retained Mac input package.

Outputs are new private paths. The existing installation, source lock and base
package remain untouched. Candidate lock adoption and native acceptance follow.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'inference'))
from neutral_paths import copy_text, neutralize_tree  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
sha = lambda f: hashlib.sha256(f.read_bytes()).hexdigest()


def verified_files(base, lock):
    if lock.get('version') != 1 or lock.get('target') != 'aarch64-apple-darwin':
        raise ValueError('Expected the reviewed Apple Silicon base lock')
    expected = {}
    for item in lock['files']:
        relative = item['path']
        path = PurePosixPath(relative)
        if path.is_absolute() or '..' in path.parts or relative in expected:
            raise ValueError('Invalid or duplicate base-lock path')
        expected[relative] = item
    actual = {}
    for f in base.rglob('*'):
        if f.is_symlink():
            raise ValueError('Symlink inputs require separate reviewed handling')
        if f.is_file():
            actual[f.relative_to(base).as_posix()] = f
    if set(actual) != set(expected):
        raise ValueError('Base package file set differs from the reviewed lock')
    for relative, f in actual.items():
        item = expected[relative]
        if f.stat().st_size != item['bytes'] or sha(f) != item['sha256']:
            raise ValueError('Base package bytes differ from the reviewed lock')
    return expected


def build_inputs(build_root, pin):
    inputs = {}
    for backend in ['cpu', 'metal']:
        directory = build_root / ('llama-' + backend)
        receipt = json.loads((directory / 'writer-build.json').read_text())
        if (receipt['llama'] != pin['llama'] or receipt['miniaudio'] != pin['miniaudio']
                or receipt['revision'] != pin['llama']['revision'] or receipt['backend'] != backend):
            raise ValueError('Text build identity differs from the reviewed source pins')
        package = directory / 'package'
        actual = {}
        for f in package.iterdir():
            if f.is_symlink() or not f.is_file():
                raise ValueError('Unexpected text package input')
            actual[f.name] = sha(f)
        if actual != receipt['files'] or 'llama-server' not in actual:
            raise ValueError('Text build package differs from its receipt')
        required = {'GGML_RPC': 'OFF', 'GGML_OPENMP': 'OFF',
                    'GGML_METAL': 'ON' if backend == 'metal' else 'OFF',
                    'CMAKE_OSX_ARCHITECTURES': 'arm64'}
        for name, value in required.items():
            flags = [v for v in receipt['configure'] if v.startswith('-D' + name + '=')]
            if flags != ['-D' + name + '=' + value]:
                raise ValueError('Text configure receipt differs from the native build policy')
        if sha(directory / 'miniaudio-LICENSE.txt') != pin['miniaudio']['license_sha256']:
            raise ValueError('Text build license differs from the source pin')
        inputs[backend] = directory
    return inputs


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--base', type=Path, required=True)
    p.add_argument('--base-lock', type=Path, required=True)
    p.add_argument('--build-root', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--lock-output', type=Path, required=True)
    a = p.parse_args()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        p.error('Assemble and verify the native package on Apple Silicon')
    base, build, output, lock_output = [v.resolve() for v in
                                      [a.base, a.build_root, a.output, a.lock_output]]
    if (output.exists() or lock_output.exists() or output.is_relative_to(ROOT)
            or any(output.is_relative_to(v) or v.is_relative_to(output) for v in [base, build])
            or any(lock_output.is_relative_to(v) for v in [ROOT, base, build, output])):
        p.error('Use new independent private package and lock output paths')
    lock = json.loads(a.base_lock.read_text())
    original = verified_files(base, lock)
    pin = json.loads((ROOT / 'scripts/inference/macos-sources.json').read_text())
    source_sbom = ROOT / 'scripts/inference/native-runtime.cdx.json'
    components = {c['bom-ref']: c for c in json.loads(source_sbom.read_text())['components']}
    if (components['llama-text']['version'] != pin['llama']['revision']
            or components['llama-text:miniaudio']['version'] != pin['miniaudio']['revision']):
        raise ValueError('Regenerate and review the source inventory for the fixed Mac pins first')
    inputs = build_inputs(build, pin)
    shutil.copytree(base, output)
    shutil.rmtree(output / 'runtime/text')
    for backend, directory in inputs.items():
        target = output / 'runtime/text' / backend
        shutil.copytree(directory / 'package', target)
        # Preserve the already reviewed native launcher; no arbitrary substitute.
        shutil.copy2(base / 'runtime/text' / backend / 'writer-worker-host', target)
        copy_text(directory / 'writer-build.json', output / 'notices' / ('llama-' + backend + '-build.json'))
    shutil.copy2(inputs['cpu'] / 'miniaudio-LICENSE.txt', output / 'notices/miniaudio-LICENSE.txt')
    shutil.copy2(ROOT / 'scripts/inference/macos-sources.json', output / 'notices/macos-sources.json')
    shutil.copy2(source_sbom, output / 'notices/native-runtime.cdx.json')
    allowed_notices = {'notices/llama-cpu-build.json', 'notices/llama-metal-build.json',
                       'notices/macos-sources.json', 'notices/miniaudio-LICENSE.txt',
                       'notices/native-runtime.cdx.json'}
    # Retained receipts from earlier builds lose their home folders too (MW-RELEASE-14).
    allowed_notices |= {'notices/' + p for p in neutralize_tree(output / 'notices')}
    entries = []
    for f in sorted(output.rglob('*')):
        if not f.is_file():
            continue
        relative, digest = f.relative_to(output).as_posix(), sha(f)
        runtime = relative.startswith('runtime/')
        if not relative.startswith('runtime/text/') and relative not in allowed_notices:
            if relative not in original or digest != original[relative]['sha256']:
                raise ValueError('An unrelated base-package input changed')
        if runtime:
            header = subprocess.check_output(['file', '-b', str(f)], text=True)
            if 'Mach-O' not in header or 'arm64' not in header:
                raise ValueError('Expected an Apple Silicon native runtime')
            for line in subprocess.check_output(['otool', '-L', str(f)], text=True).splitlines()[1:]:
                dependency = line.strip().split(' (')[0]
                if not dependency.startswith(('@rpath/', '@loader_path/', '/usr/lib/', '/System/Library/')):
                    raise ValueError('Runtime has a dependency outside the portable package')
        entries.append({'path': relative, 'bytes': f.stat().st_size, 'sha256': digest,
                        'role': 'runtime' if runtime else 'notice',
                        **({'executable': True} if runtime and f.suffix != '.dylib' else {})})
    verified_files(base, lock)
    result = {**lock, 'files': entries,
              'provenance': 'MW-SEC-30: all non-text runtime bytes retained from the reviewed MW-SEC-28 base '
              'lock SHA-256 ' + sha(a.base_lock) + '. The complete text subtree is rebuilt natively for '
              'Apple Silicon CPU/Metal from llama.cpp ' + pin['llama']['revision'] + ' with miniaudio '
              + pin['miniaudio']['revision'] + '. Exact source/license/build inputs are in notices; '
              'the existing speech and FFmpeg source/patch attributions remain in that inventory. '
              'Model weights excluded.'}
    lock_output.parent.mkdir(parents=True, exist_ok=True)
    with lock_output.open('x') as f:
        f.write(json.dumps(result, indent=2) + '\n')
    print('Prepared', len(entries), 'candidate package files; native acceptance and lock adoption remain required.')


if __name__ == '__main__':
    main()
