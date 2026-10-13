#!/usr/bin/env python3
"""Qualify real Super/Command policies with pinned native Windows Qwen/llama.cpp.
This is a developer qualification harness, not an installed Writer provider.
"""
import argparse
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import zipfile
from qualify import ROOT, fetch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=Path, required=True)
    parser.add_argument('--tests', type=Path, required=True, help='Windows API lib test executable')
    parser.add_argument('--backend', choices=['cpu', 'vulkan'], required=True)
    args = parser.parse_args()
    cache = args.cache.resolve()
    if cache.is_relative_to(ROOT): parser.error('Use a private cache outside source')
    cache.mkdir(parents=True, exist_ok=True)
    name = f'llama-b10809-bin-win-{args.backend}-x64.zip'
    size, sha = {
        'cpu': (18407457, '9df3158ed228a641a4b127942d7f459f24c9e13f04682659d05c00c80099b6b5'),
        'vulkan': (35221385, '97e50b3ef0cdd2cb4d5afd446a9006b3496bee6c0d0ba7083d32f36075771870'),
    }[args.backend]
    archive = fetch({'file_name': name, 'bytes': size, 'sha256': sha,
                     'source_url': 'https://github.com/ggml-org/llama.cpp/releases/download/b10809/'+name}, cache, [])
    runtime = cache/args.backend
    with zipfile.ZipFile(archive) as z:
        for item in z.infolist():
            path = PurePosixPath(item.filename.replace('\\', '/'))
            if path.is_absolute() or '..' in path.parts or ':' in str(path) or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError('Unsafe archive path')
            destination = runtime/Path(*path.parts)
            if item.filename.endswith(('/', '\\')):
                destination.mkdir(parents=True, exist_ok=True)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                with z.open(item) as source, destination.open('wb') as output:
                    shutil.copyfileobj(source, output)
    model = fetch({'file_name': 'qwen3-4b-instruct-q4.gguf', 'bytes': 2497280448,
                   'sha256': '8cdb57cbb880d313736a9bc4e3d3d2485f145b5e19cf33783746e753e82641fc',
                   'source_url': 'https://huggingface.co/lmstudio-community/Qwen3-4B-Instruct-2507-GGUF/resolve/4edb920b6f14e3b9284d4502a6485103d72cde05/Qwen3-4B-Instruct-2507-Q4_K_M.gguf'}, cache, [])
    def native(path):
        return str(path.resolve()) if os.name == 'nt' else subprocess.check_output(['wslpath', '-w', str(path.resolve())], text=True).strip()
    powershell = 'powershell.exe' if os.name == 'nt' else '/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
    subprocess.run([powershell, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', native(Path(__file__).with_name('smoke-text-modes.ps1')),
                    '-RuntimeDirectory', native(runtime), '-ModelPath', native(model), '-TestExecutable', native(args.tests),
                    '-OutputDirectory', native(cache/(args.backend+'-modes')), '-Backend', args.backend], check=True, timeout=1500)


if __name__ == '__main__': main()
