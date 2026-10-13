#!/usr/bin/env python3
"""Build the staged Writer whisper fork natively for Apple Silicon Metal or CPU."""
import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags, roots, write_receipt  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--backend', choices=['metal', 'cpu'], required=True)
    parser.add_argument('--cmake', default='cmake')
    parser.add_argument('--jobs', type=int, default=4)
    args = parser.parse_args()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        parser.error('Build on an Apple Silicon Mac')
    source, output = args.source.resolve(), args.output.resolve()
    if not (source / 'writer-source.txt').is_file():
        parser.error('Use stage-whisper.py first')
    settings = {
        'CMAKE_BUILD_TYPE': 'Release', 'CMAKE_OSX_ARCHITECTURES': 'arm64',
        'CMAKE_OSX_DEPLOYMENT_TARGET': '15.0', 'BUILD_SHARED_LIBS': 'ON',
        'CMAKE_INSTALL_RPATH': '@loader_path', 'CMAKE_BUILD_WITH_INSTALL_RPATH': 'ON',
        'WHISPER_BUILD_TESTS': 'OFF', 'WHISPER_BUILD_EXAMPLES': 'ON',
        'GGML_NATIVE': 'OFF', 'GGML_BACKEND_DL': 'OFF',
        'GGML_METAL': 'ON' if args.backend == 'metal' else 'OFF',
        'GGML_METAL_EMBED_LIBRARY': 'ON', 'GGML_ACCELERATE': 'ON',
        'GGML_VULKAN': 'OFF', 'WHISPER_COREML': 'OFF',
    }
    # No build-machine paths in __FILE__ strings (MW-RELEASE-14).
    mapped = prefix_flags(roots(whisper_source=source, whisper_build=output))
    for language in ('C', 'CXX', 'OBJC'):
        settings[f'CMAKE_{language}_FLAGS'] = ' '.join(mapped)
    command = [args.cmake, '-S', str(source), '-B', str(output)]
    command += [f'-D{k}={v}' for k, v in settings.items()]
    subprocess.run(command, check=True)
    subprocess.run([args.cmake, '--build', str(output), '--target', 'whisper-server',
                    '--parallel', str(args.jobs)], check=True)
    package = output / 'package'
    package.mkdir(exist_ok=False)
    shutil.copy2(output / 'bin/whisper-server', package)
    for library in output.rglob('*.dylib'):
        if package not in library.parents:
            shutil.copy2(library, package / library.name)
    subprocess.run(['cc', '-O2', '-Wall', '-Wextra', '-Werror', '-mmacosx-version-min=15.0', *prefix_flags(roots()),
                    str(Path(__file__).with_name('worker-host-macos.c')), '-o',
                    str(package / 'writer-worker-host')], check=True)
    receipt = {'source': (source / 'writer-source.txt').read_text(),
               'backend': args.backend, 'configure': command,
               'files': {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                         for p in sorted(package.iterdir())}}
    write_receipt(output / 'writer-build.json', receipt)
    print(package)


if __name__ == '__main__':
    main()
