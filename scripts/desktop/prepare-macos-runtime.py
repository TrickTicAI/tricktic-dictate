#!/usr/bin/env python3
"""Assemble native MW-51 build outputs; verify all bytes against the reviewed lock.
--write-lock is an explicit maintainer operation for reviewing a new package.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'inference'))
from neutral_paths import copy_text, neutralize_tree  # noqa: E402
ROOT = Path(__file__).resolve().parents[2]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--build-root', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--write-lock', action='store_true')
    args = p.parse_args()
    build, output = args.build_root.resolve(), args.output.resolve()
    if output.is_relative_to(ROOT):
        p.error('Keep binaries and model weights outside source')
    output.mkdir(parents=True, exist_ok=False)
    def copy(source, target):
        target = output / target
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    def tree(source, target):
        for file in sorted(source.rglob('*')):
            if file.is_file():
                copy(file, Path(target)/file.relative_to(source))
    # Build the host from the task's reviewed source, never accept an arbitrary launcher.
    host = output / 'writer-worker-host'
    subprocess.run(['cc', '-O2', '-Wall', '-Wextra', '-Werror', '-mmacosx-version-min=15.0',
                    str(ROOT/'scripts/inference/worker-host-macos.c'), '-o', str(host)], check=True)
    for backend in ['metal', 'cpu']:
        whisper = Path('runtime/whisper') / ('cpu' if backend == 'cpu' else '')
        text = Path('runtime/text') / backend
        nemo = Path('runtime/nemo') / backend
        tree(build/f'whisper-{backend}/package', whisper)
        tree(build/f'llama-{backend}/package', text)
        tree(build/f'nemo-{backend}/nemo-speech/bin', nemo/'bin')
        for library in (build/f'nemo-{backend}/nemo-speech/lib').glob('*.dylib'):
            copy(library, nemo/'lib'/library.name)
        # NeMo embeds its Metal library in its ggml dylib too.
        tree(build/f'nemo-{backend}/nemo-speech/share/licenses', f'notices/nemo-{backend}')
        for target in [whisper, text, nemo/'bin']:
            copy(host, target/'writer-worker-host')
        copy_text(build/f'whisper-{backend}/writer-build.json', output/f'notices/whisper-{backend}-build.json')
        copy_text(build/f'llama-{backend}/writer-build.json', output/f'notices/llama-{backend}-build.json')
    ffmpeg_pin = json.loads((ROOT/'scripts/inference/ffmpeg-source.json').read_text())
    ffmpeg_source = build/'ffmpeg-build'/('ffmpeg-'+ffmpeg_pin['version'])
    copy(ffmpeg_source/'ffmpeg', 'runtime/ffmpeg/ffmpeg')
    copy(host, 'runtime/ffmpeg/writer-worker-host')
    host.unlink()
    copy(ffmpeg_source/'COPYING.LGPLv2.1', 'notices/ffmpeg-LICENSE.txt')
    copy(ffmpeg_source/'LICENSE.md', 'notices/ffmpeg-LICENSE.md')
    copy_text(ffmpeg_source/'ffbuild/config.mak', output/'notices/ffmpeg-config.txt')
    copy_text(build/'ffmpeg-build/writer-build.json', output/'notices/ffmpeg-build.json')
    for name in ['ffmpeg-source.json', 'native-runtime.cdx.json']:
        copy(ROOT/'scripts/inference'/name, 'notices/'+name)
    copy(build/'llama-source/LICENSE', 'notices/llama-LICENSE.txt')
    copy(build/'whisper-source/LICENSE', 'notices/whisper-LICENSE.txt')
    copy(build/'whisper-source/writer-source.txt', 'notices/writer-whisper-source.txt')
    copy(ROOT/'scripts/inference/macos-sources.json', 'notices/macos-sources.json')
    copy(ROOT/'scripts/inference/worker-host-macos.c', 'notices/writer-worker-host.c')
    entries = []
    for file in sorted(output.rglob('*')):
        if not file.is_file():
            continue
        relative = file.relative_to(output).as_posix()
        runtime = relative.startswith('runtime/')
        if runtime:
            # No package may depend on the build machine's Homebrew or /tmp paths.
            header = subprocess.check_output(['file', '-b', str(file)], text=True)
            if 'Mach-O' not in header or 'arm64' not in header:
                raise ValueError('Expected an arm64 Mach-O runtime: '+relative)
            for line in subprocess.check_output(['otool', '-L', str(file)], text=True).splitlines()[1:]:
                dependency = line.strip().split(' (')[0]
                if not dependency.startswith(('@rpath/', '@loader_path/', '/usr/lib/', '/System/Library/')):
                    raise ValueError('Nonportable dependency: '+relative+' -> '+dependency)
        entries.append({'path': relative, 'bytes': file.stat().st_size,
                        'sha256': hashlib.file_digest(file.open('rb'), 'sha256').hexdigest(),
                        'role': 'runtime' if runtime else 'notice',
                        **({'executable': True} if runtime and file.suffix != '.dylib' else {})})
    lock = {'version': 1, 'target': 'aarch64-apple-darwin',
            'provenance': 'Apple Silicon macOS 15+. Writer whisper 371b5a7 with model-input guards, embedded Metal and isolated CPU; llama.cpp b10809 Metal/CPU; NeMo v0.1.0 release archives; static audio-only LGPL FFmpeg 8.0.3. Exact source and build inputs are in notices. Model weights are excluded.',
            'files': entries}
    lock_path = ROOT/'scripts/desktop/macos-runtime.lock.json'
    if args.write_lock:
        lock_path.write_text(json.dumps(lock, indent=2)+'\n')
    else:
        expected = json.loads(lock_path.read_text())
        if expected['files'] != entries:
            raise ValueError('Package differs from the reviewed macOS lock; retain outputs for review')
    print(f'Prepared {len(entries)} pinned runtime/notice files at {output}')


if __name__ == '__main__':
    main()
