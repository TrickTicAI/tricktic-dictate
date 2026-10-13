#!/usr/bin/env python3
"""Build the pinned CPU benchmark outside source (Linux NDK host)."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from device import ASSETS, ROOT, digest, private_path


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True, help='Clean checkout of the pinned whisper.cpp revision')
    p.add_argument('--output', type=Path, required=True, help='New private build directory')
    p.add_argument('--ndk', type=Path, required=True)
    p.add_argument('--abi', choices=['arm64-v8a', 'x86_64'], default='arm64-v8a')
    p.add_argument('--jobs', type=int, default=4)
    p.add_argument('--armv82', action='store_true', help='Require ARMv8.2 FP16/dot-product instructions (checked on device)')
    a = p.parse_args()
    if a.armv82 and a.abi != 'arm64-v8a':
        p.error('--armv82 requires arm64-v8a')
    source = private_path(a.source)
    output = private_path(a.output)
    pin = ASSETS['runtime']
    if run('git', '-C', str(source), 'rev-parse', 'HEAD') != pin['revision']:
        p.error('Wrong runtime revision; use assets.json')
    if run('git', '-C', str(source), 'status', '--porcelain'):
        p.error('Runtime source is dirty')
    if f'Pkg.Revision = {pin["ndk"]}' not in (a.ndk / 'source.properties').read_text():
        p.error('Wrong NDK revision')
    output.mkdir(parents=True, exist_ok=False)
    command = ['cmake', '-S', str(ROOT), '-B', str(output),
               '-DCMAKE_BUILD_TYPE=Release', f'-DWHISPER_SOURCE={source}',
               f'-DCMAKE_TOOLCHAIN_FILE={a.ndk.resolve()}/build/cmake/android.toolchain.cmake',
               f'-DANDROID_ABI={a.abi}', f'-DANDROID_PLATFORM=android-{pin["android_api"]}',
               '-DANDROID_STL=c++_static']
    if a.armv82:
        command.append('-DGGML_CPU_ARM_ARCH=armv8.2-a+fp16+dotprod')
    subprocess.run(command, check=True)
    subprocess.run(['cmake', '--build', str(output), '--target', 'writer-android-bench', '--parallel', str(a.jobs)], check=True)
    binary = output / 'writer-android-bench'
    receipt = {'runtime': pin, 'abi': a.abi, 'required_cpu_features': ['asimdhp', 'asimddp'] if a.armv82 else [], 'binary_sha256': digest(binary),
               'command': command, 'cmake': run('cmake', '--version'),
               'compiler': run(str(a.ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin/clang'), '--version'),
               'harness_sha256': {f: digest(ROOT / f) for f in ['main.cpp', 'CMakeLists.txt', 'build.py']}}
    (output / 'build.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(f'Built {a.abi}: {binary}')


if __name__ == '__main__':
    os.umask(0o077)
    main()
