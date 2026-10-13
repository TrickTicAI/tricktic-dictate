#!/usr/bin/env python3
"""Assemble reviewed FFmpeg/Whisper builds over a byte-verified retained package.
Writes a candidate lock beside the output; never silently edits the approved lock.
"""
import argparse
import hashlib
import json
import sys
from pathlib import Path
import shutil
import subprocess

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import copy_text, neutralize_tree  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify(root, entries):
    expected = set()
    for item in entries:
        rel = Path(item['path'])
        if rel.is_absolute() or '..' in rel.parts or item['path'] in expected:
            raise ValueError('Invalid or duplicate lock path')
        expected.add(item['path'])
        path = root/rel
        if (not path.is_file() or any((root/Path(*rel.parts[:i])).is_symlink() for i in range(1, len(rel.parts)+1))
                or path.stat().st_size != item['bytes'] or digest(path) != item['sha256']):
            raise ValueError('Package differs from lock: '+item['path'])
    actual = {p.relative_to(root).as_posix() for name in ['runtime', 'notices'] for p in (root/name).rglob('*') if p.is_file()}
    if actual != expected:
        raise ValueError('Package has unexpected/missing files: '+str(actual ^ expected))


def verify_whisper(build, source, backend, target):
    evidence = json.loads((build/'writer-build.json').read_text())
    if evidence['source'] != (source/'writer-source.txt').read_text() or evidence['backend'] != backend:
        raise ValueError('Whisper build source/backend differs from requested source/backend')
    # The receipt must describe the actual staged patch bytes, not just its label.
    for line in evidence['source'].splitlines():
        name, value = line.split('=', 1)
        if name == 'patched_server_sha256' and digest(source/'examples/server/server.cpp') != value:
            raise ValueError('Whisper staged server differs from receipt')
        if '/' in name and digest(source/name) != value:
            raise ValueError('Whisper staged source differs from receipt: '+name)
    package = build/('bin' if target == 'windows' else 'package')
    files = {f.name: f for f in package.iterdir()
             if f.is_file() and (target != 'windows' or f.suffix in ['.exe', '.dll'])}
    server = 'whisper-server.exe' if target == 'windows' else 'whisper-server'
    if server not in files or set(files) != set(evidence['files']):
        raise ValueError('Whisper package is incomplete or has unexpected files')
    for name, file in files.items():
        if file.is_symlink() or digest(file) != evidence['files'][name]:
            raise ValueError('Whisper binary differs from its build receipt: '+name)
    return files


def verify_macos_runtime(root):
    for file in (root/'runtime').rglob('*'):
        if not file.is_file():
            continue
        header = subprocess.check_output(['file', '-b', str(file)], text=True)
        if 'Mach-O' not in header or 'arm64' not in header:
            raise ValueError('Expected arm64 Mach-O runtime: '+str(file))
        for line in subprocess.check_output(['otool', '-L', str(file)], text=True).splitlines()[1:]:
            dependency = line.strip().split(' (')[0]
            if not dependency.startswith(('@rpath/', '@loader_path/', '/usr/lib/', '/System/Library/')):
                raise ValueError('Nonportable runtime dependency: '+dependency)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--base', type=Path, required=True)
    p.add_argument('--base-lock', type=Path, required=True)
    p.add_argument('--build-root', type=Path, required=True)
    p.add_argument('--ffmpeg-build', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--target', choices=['windows', 'macos'], required=True)
    args = p.parse_args()
    old = json.loads(args.base_lock.read_text())
    target = 'x86_64-pc-windows-msvc' if args.target == 'windows' else 'aarch64-apple-darwin'
    # The original Windows lock predates the target field.
    if old.get('target', 'x86_64-pc-windows-msvc') != target:
        raise ValueError('Base lock target differs from requested target')
    verify(args.base, old['files'])
    pin = json.loads((ROOT/'scripts/inference/ffmpeg-source.json').read_text())
    receipt = json.loads((args.ffmpeg_build/'writer-build.json').read_text())
    binary = args.ffmpeg_build/('ffmpeg-'+pin['version'])/('ffmpeg.exe' if args.target == 'windows' else 'ffmpeg')
    if receipt['source'] != pin or receipt['target'] != args.target or receipt['sanitizers']:
        raise ValueError('Wrong FFmpeg source, target or sanitizer build')
    if receipt['binary'] != {'sha256': digest(binary), 'bytes': binary.stat().st_size}:
        raise ValueError('FFmpeg differs from its build receipt')
    output = args.output.resolve()
    if output.is_relative_to(ROOT):
        p.error('Keep binary packages outside source')
    output.mkdir(parents=True, exist_ok=False)
    def copy(source, relative):
        dest = output/relative; dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
    for item in old['files']:
        path = item['path']
        if path.startswith('runtime/whisper/') or path.startswith('runtime/ffmpeg/') and not path.endswith('writer-worker-host'):
            continue
        copy(args.base/path, path)
    copy(binary, 'runtime/ffmpeg/'+binary.name)
    for name, dest in [('COPYING.LGPLv2.1', 'ffmpeg-LICENSE.txt'), ('LICENSE.md', 'ffmpeg-LICENSE.md')]:
        copy(binary.parent/name, 'notices/'+dest)
    # Receipts keep the configuration but not the builder's home folder (MW-RELEASE-14).
    copy_text(binary.parent/'ffbuild/config.mak', output/'notices/ffmpeg-config.txt')
    copy_text(args.ffmpeg_build/'writer-build.json', output/'notices/ffmpeg-build.json')
    for backend in (['cpu', 'vulkan'] if args.target == 'windows' else ['cpu', 'metal']):
        build = args.build_root/('whisper-'+backend)
        files = verify_whisper(build, args.build_root/'whisper-hardened', backend, args.target)
        destination = 'runtime/whisper/' + ('cpu/' if backend == 'cpu' else '')
        for file in sorted(files.values()):
            copy(file, destination+file.name)
        copy_text(build/'writer-build.json', output/('notices/whisper-'+backend+'-build.json'))
        copy_text(build/'CMakeCache.txt', output/('notices/whisper-'+backend+'-cmake.txt'))
        compiler_info = list(build.glob('CMakeFiles/*/CMakeCXXCompiler.cmake'))
        if len(compiler_info) != 1:
            raise ValueError('Expected one CMake compiler identification receipt')
        copy_text(compiler_info[0], output/('notices/whisper-'+backend+'-compiler.cmake'))
    copy(args.build_root/'whisper-hardened/writer-source.txt', 'notices/writer-whisper-source.txt')
    for name in ['ffmpeg-source.json', 'native-runtime.cdx.json']:
        copy(ROOT/'scripts/inference'/name, 'notices/'+name)
    neutralize_tree(output/'notices')
    if args.target == 'macos':
        copy(ROOT/'scripts/inference/macos-sources.json', 'notices/macos-sources.json')
        verify_macos_runtime(output)
    entries = []
    for file in sorted(output.rglob('*')):
        if not file.is_file(): continue
        relative = file.relative_to(output).as_posix()
        runtime = relative.startswith('runtime/')
        entries.append({'path': relative, 'bytes': file.stat().st_size, 'sha256': digest(file),
                        'role': 'runtime' if runtime else 'notice',
                        **({'executable': True} if args.target == 'macos' and runtime and file.suffix != '.dylib' else {})})
    lock = {**old, 'target': target, 'files': entries,
            'provenance': 'MW-SEC-9: signature-verified FFmpeg 8.0.3 built from source with audio-only configuration; Writer whisper 371b5a7561823ab2bb32142d2751e35e7534727b plus private-worker authentication, scheduling, encoder and model-input guards; pinned cpp-httplib 0.58.0 and writer-http-limits.h bound pre-auth lines/headers, idle time, threads and queue (MW-SEC-28). Source/patch SBOM and configure receipts in notices. NeMo v0.1.0 (4f9676226f667d14608487df744f375db87127f8) and llama.cpp b10809 (5266f24da75dc449bd56cbed7addb9c8e4a6a73e) retain the approved base-package bytes (macOS llama source-build receipts retained; upstream release compiler toolchains are not independently attested). Model weights excluded.'}
    verify(output, entries)
    output.with_suffix('.lock.json').write_text(json.dumps(lock, indent=2)+'\n')
    print(f'Verified {len(entries)} package inputs; candidate lock: {output.with_suffix(".lock.json")}')


if __name__ == '__main__':
    main()
