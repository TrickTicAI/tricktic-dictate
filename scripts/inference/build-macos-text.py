#!/usr/bin/env python3
"""Build the staged fixed CPU/Metal text runtime on Apple Silicon."""
import argparse
import hashlib
import json
import sys
from pathlib import Path
import platform
import shutil
import subprocess

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags, roots, write_receipt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def validate_source(source, pin):
    """Verify actual Git/file inputs, including provenance supplied by staging."""
    revision = pin['llama']['revision']
    git = ['git', '-C', str(source)]
    if subprocess.check_output(git + ['rev-parse', 'HEAD'], text=True).strip() != revision:
        raise ValueError('Expected the pinned staged upstream revision')
    staged = json.loads((source / 'writer-source.json').read_text())
    if staged['llama'] != pin['llama'] or staged['miniaudio'] != pin['miniaudio']:
        raise ValueError('Source receipt differs from the reviewed pins')
    for path, expected in [('vendor/miniaudio/miniaudio.h', pin['miniaudio']['sha256']),
                           ('writer-miniaudio-LICENSE.txt', pin['miniaudio']['license_sha256'])]:
        if hashlib.sha256((source / path).read_bytes()).hexdigest() != expected:
            raise ValueError('Staged source/license differs from the reviewed pin')
    original = subprocess.check_output(git + ['show', revision + ':vendor/miniaudio/miniaudio.h'])
    if staged['vendored_miniaudio_original_sha256'] != hashlib.sha256(original).hexdigest():
        raise ValueError('Original-header provenance differs from the pinned Git object')
    changed = subprocess.check_output(git + ['diff', '--name-only', revision], text=True).splitlines()
    if set(changed) != {'vendor/miniaudio/miniaudio.h'}:
        raise ValueError('Only the reviewed miniaudio replacement may change upstream source')
    # Include ignored files: an upstream .gitignore is not approval to inject
    # additional build inputs into the staged source tree.
    extra = subprocess.check_output(git + ['ls-files', '--others'], text=True).splitlines()
    if set(extra) != {'writer-source.json', 'writer-miniaudio-LICENSE.txt'}:
        raise ValueError('Unexpected untracked source input')
    return staged


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--backend', choices=['metal', 'cpu'], required=True)
    p.add_argument('--jobs', type=int, default=4)
    args = p.parse_args()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        p.error('Build on an Apple Silicon Mac')
    source, output = args.source.resolve(), args.output.resolve()
    pin = json.loads((ROOT / 'scripts/inference/macos-sources.json').read_text())
    revision = pin['llama']['revision']
    staged = validate_source(source, pin)
    if output.exists() or output.is_relative_to(source) or output.is_relative_to(ROOT):
        p.error('Use a new private build path outside source')
    settings = {'CMAKE_BUILD_TYPE': 'Release', 'CMAKE_OSX_ARCHITECTURES': 'arm64',
                'CMAKE_OSX_DEPLOYMENT_TARGET': '15.0', 'CMAKE_INSTALL_RPATH': '@loader_path',
                'CMAKE_BUILD_WITH_INSTALL_RPATH': 'ON', 'BUILD_SHARED_LIBS': 'ON',
                'GGML_METAL': 'ON' if args.backend == 'metal' else 'OFF',
                'GGML_METAL_EMBED_LIBRARY': 'ON', 'GGML_NATIVE': 'OFF',
                'GGML_RPC': 'OFF', 'GGML_OPENMP': 'OFF',
                'CMAKE_EXPORT_COMPILE_COMMANDS': 'ON',
                'LLAMA_BUILD_TESTS': 'OFF', 'LLAMA_BUILD_EXAMPLES': 'OFF',
                'LLAMA_BUILD_TOOLS': 'ON', 'LLAMA_BUILD_SERVER': 'ON',
                'LLAMA_CURL': 'OFF', 'LLAMA_OPENSSL': 'OFF'}
    # No build-machine paths in __FILE__ strings (MW-RELEASE-14).
    mapped = ' '.join(prefix_flags(roots(llama_source=source, llama_build=output)))
    settings.update(CMAKE_C_FLAGS=mapped, CMAKE_CXX_FLAGS=mapped, CMAKE_OBJC_FLAGS=mapped)
    command = ['cmake', '-S', str(source), '-B', str(output)] + [f'-D{k}={v}' for k,v in settings.items()]
    subprocess.run(command, check=True)
    subprocess.run(['cmake', '--build', str(output), '--target', 'llama-server', '--parallel', str(args.jobs)], check=True)
    validate_source(source, pin)
    package = output / 'package'
    package.mkdir(exist_ok=False)
    shutil.copy2(output / 'bin/llama-server', package)
    for library in output.rglob('*.dylib'):
        if package not in library.parents:
            shutil.copy2(library, package / library.name)
    receipt = {'revision': revision, 'llama': pin['llama'], 'miniaudio': pin['miniaudio'],
               'vendored_miniaudio_original_sha256': staged['vendored_miniaudio_original_sha256'],
               'backend': args.backend, 'configure': command,
               'compiler': subprocess.check_output(['cc', '--version'], text=True),
               'files': {f.name: hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted(package.iterdir())}}
    write_receipt(output / 'writer-build.json', receipt)
    shutil.copy2(source / 'writer-miniaudio-LICENSE.txt', output / 'miniaudio-LICENSE.txt')
    print(package)


if __name__ == '__main__':
    main()
