#!/usr/bin/env python3
"""Build Writer's pinned, audio-only FFmpeg in a fresh directory.

Windows uses the prepared LLVM/MSVC SDK on Linux; macOS uses native clang.
Linux is provided for ASan/UBSan media regression testing, not distribution.
"""
import argparse
import hashlib
import json
import os
import sys
from pathlib import Path
import platform
import shlex
import subprocess
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags, roots, write_receipt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PIN = Path(__file__).with_name('ffmpeg-source.json')


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def flags():
    # Match local_stt's five fixed input demuxers and PCM outputs. No network,
    # devices, video, subtitle, external codec libraries or autodetected features.
    return ['--disable-everything', '--disable-autodetect', '--disable-network',
            '--disable-shared', '--enable-static', '--disable-doc', '--disable-debug',
            '--disable-ffplay', '--disable-ffprobe', '--disable-avdevice',
            '--disable-swscale', '--disable-x86asm', '--enable-ffmpeg',
            '--enable-protocol=file,pipe', '--enable-demuxer=matroska,ogg,mov,wav,mp3',
            '--enable-decoder=aac,alac,flac,mp3,opus,vorbis,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_f64le,pcm_u8',
            '--enable-parser=aac,flac,mpegaudio,opus,vorbis',
            '--enable-encoder=pcm_s16le', '--enable-muxer=wav,pcm_s16le',
            '--enable-filter=aresample,aformat,anull']


def verify_archive(archive, pin):
    if archive.stat().st_size != pin['bytes'] or sha256(archive) != pin['sha256']:
        raise ValueError('FFmpeg archive differs from the signature-verified source pin')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target', choices=['windows', 'macos', 'linux'], required=True)
    parser.add_argument('--sanitize', action='store_true')
    parser.add_argument('--jobs', type=int, default=4)
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error('--jobs must be positive')
    if args.sanitize and args.target != 'linux':
        parser.error('Sanitizers are qualified on Linux only')
    if args.target == 'macos' and (platform.system(), platform.machine()) != ('Darwin', 'arm64'):
        parser.error('Build macOS on Apple Silicon')
    pin = json.loads(PIN.read_text())
    archive, output = args.archive.resolve(), args.output.resolve()
    verify_archive(archive, pin)
    if output.is_relative_to(ROOT):
        parser.error('Keep native build outputs outside source')
    output.mkdir(parents=True, exist_ok=False)
    with tarfile.open(archive) as tar:
        tar.extractall(output, filter='data')
    source = output / ('ffmpeg-' + pin['version'])
    command = ['./configure'] + flags()
    compiler = ['cc']
    env = os.environ.copy()
    wrappers = {}
    if args.target == 'windows':
        llvm = Path(os.environ['WRITER_LLVM_ROOT']).resolve()
        sdk = Path(os.environ['WRITER_WINDOWS_SDK']).resolve()
        compiler = [str(llvm/'bin/clang-cl')]
        mapped = prefix_flags(roots(ffmpeg_build=output, llvm=llvm, windows_sdk=sdk), clang_cl=True)
        includes = [sdk/'crt/include'] + [sdk/'sdk/include'/p for p in ['ucrt', 'um', 'shared', 'winrt']]
        libraries = [sdk/'crt/lib/x86_64', sdk/'sdk/lib/ucrt/x86_64', sdk/'sdk/lib/um/x86_64']
        wrappers = {
            'target-cc': compiler + ['--target=x86_64-pc-windows-msvc'] + [f'/imsvc{p}' for p in includes] + mapped,
            'target-ld': [str(llvm/'bin/lld-link')] + [f'/libpath:{p}' for p in libraries],
        }
        for name, argv in wrappers.items():
            wrapper = output / name
            wrapper.write_text('#!/bin/sh\nexec ' + shlex.join(argv) + ' "$@"\n')
            wrapper.chmod(0o700)
        # clang-cl runs on Linux and emits Linux include paths. FFmpeg otherwise
        # sends each one through WSL's Windows-path conversion during dependency generation.
        path_wrapper = output/'wslpath'
        path_wrapper.write_text('#!/bin/sh\n[ "$1" = -u ] && shift\ncase "$1" in\n /*) printf "%s\\n" "$1";;\n *) exec /usr/bin/wslpath -u "$1";;\nesac\n')
        path_wrapper.chmod(0o700)
        # Tools by name from PATH: FFmpeg embeds this configure line in ffmpeg.exe.
        command += ['--target-os=win32', '--arch=x86_64', '--enable-cross-compile',
                    '--toolchain=msvc', '--cc=target-cc', '--ld=target-ld',
                    '--ar=llvm-ar', '--nm=llvm-nm', '--windres=false']
        env['PATH'] = str(output) + os.pathsep + str(llvm/'bin') + os.pathsep + env['PATH']
    elif args.target == 'macos':
        compiler = ['clang']
        command += ['--cc=clang', '--extra-cflags=-mmacosx-version-min=15.0',
                    '--extra-ldflags=-mmacosx-version-min=15.0']
    if args.sanitize:
        command += ['--extra-cflags=-fsanitize=address,undefined -fno-omit-frame-pointer -g',
                    '--extra-ldflags=-fsanitize=address,undefined', '--disable-stripping']
    subprocess.run(command, cwd=source, env=env, check=True)
    binary = source / ('ffmpeg.exe' if args.target == 'windows' else 'ffmpeg')
    subprocess.run(['make', f'-j{args.jobs}', binary.name], cwd=source, env=env, check=True)
    receipt = {'schema_version': 1, 'source': pin, 'target': args.target,
               'compiler': subprocess.check_output(compiler + ['--version'], text=True, env=env).strip(),
               'configure': command, 'compiler_wrappers': wrappers,
               'sanitizers': ['address', 'undefined'] if args.sanitize else [],
               'config_sha256': sha256(source/'config.h'),
               'binary': {'sha256': sha256(binary), 'bytes': binary.stat().st_size}}
    write_receipt(output/'writer-build.json', receipt)
    print(binary)


if __name__ == '__main__':
    main()
