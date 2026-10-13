#!/usr/bin/env python3
"""Fixture test: Whisper is replaced, unused Windows components are pruned, legacy workers and drifted bases are refused."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('replace-windows-speech.py')
AUTH = Path(__file__).resolve().parents[2] / 'runtime/whisper/writer-auth.h'


def pe(extra=b''):
    header = bytearray(256)
    header[:2] = b'MZ'
    header[60:64] = (64).to_bytes(4, 'little')
    header[64:70] = b'PE\0\0\x64\x86'
    return bytes(header) + extra


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class ReplaceWindowsSpeech(unittest.TestCase):
    def fixture(self, root, server=b'writer: private listener X-Writer-Instance'):
        base = root / 'base'
        for rel, data in {'runtime/whisper/whisper-server.exe': pe(b'legacy'), 'runtime/whisper/ggml-cpu.dll': pe(),
                          'runtime/nemo/cpu/bin/nemo-speech.exe': pe(b'nemo'), 'notices/whisper.cpp-LICENSE.txt': b'MIT',
                          'runtime/nemo/cpu/bin/msvcp140.dll': pe(b'vc'), 'notices/nemo-cpu/nemo-speech/LICENSE': b'MIT',
                          'runtime/text/cpu/llama-server.exe': pe(b'server'), 'runtime/text/cpu/llama-server-impl.dll': pe(b'impl'),
                          'runtime/text/cpu/llama-cli-impl.dll': pe(b'cli'), 'runtime/text/vulkan/ggml-rpc.dll': pe(b'rpc'),
                          'notices/writer-whisper-source.txt': b'old', 'notices/whisper-cpu-cmake.txt': b'old'}.items():
            write(base / rel, data)
        files = [{'path': p.relative_to(base).as_posix(), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest(),
                  'bytes': p.stat().st_size, 'role': 'runtime' if 'runtime' in p.parts else 'notice'}
                 for p in sorted(base.rglob('*')) if p.is_file()]
        (root / 'base.lock.json').write_text(json.dumps({'version': 1, 'provenance': 'old', 'files': files}))
        build = root / 'build'
        write(build / 'whisper-source/LICENSE', b'MIT')
        write(build / 'whisper-source/examples/server/writer-auth.h', AUTH.read_bytes())
        receipt = 'upstream=x\nexamples/server/writer-auth.h=' + hashlib.sha256(AUTH.read_bytes()).hexdigest() + '\n'
        (build / 'whisper-source/writer-source.txt').write_text(receipt)
        for backend in ['cpu', 'vulkan']:
            names = ['whisper.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu-x64.dll'] + (['ggml-vulkan.dll'] if backend == 'vulkan' else [])
            for name in names:
                write(build / f'whisper-{backend}/bin' / name, pe(backend.encode()))
            write(build / f'whisper-{backend}/bin/whisper-server.exe', pe(server + backend.encode()))
            write(build / f'whisper-{backend}/bin/vulkan-shaders-gen', b'\x7fELF host tool')
            write(build / f'whisper-{backend}/CMakeCache.txt', b'cache ' + backend.encode())
            write(build / f'whisper-{backend}/CMakeFiles/3.31/CMakeCXXCompiler.cmake', b'clang-cl')
            (build / f'whisper-{backend}/writer-build.json').write_text(json.dumps({'source': receipt, 'backend': backend}))
        return base

    def run_script(self, root):
        return subprocess.run([sys.executable, str(SCRIPT), '--base', str(root / 'base'), '--base-lock', str(root / 'base.lock.json'),
                               '--build-root', str(root / 'build'), '--output', str(root / 'out'),
                               '--lock-output', str(root / 'out.lock.json')], capture_output=True, text=True)

    def test_replaces_only_whisper(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            result = self.run_script(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            lock = {e['path']: e for e in json.loads((root / 'out.lock.json').read_text())['files']}
            old = {e['path']: e for e in json.loads((root / 'base.lock.json').read_text())['files']}
            self.assertFalse([p for p in lock if 'nemo' in p], 'NeMo and its vendor DLLs are not shipped')
            self.assertNotIn('runtime/text/cpu/llama-cli-impl.dll', lock)
            self.assertNotIn('runtime/text/vulkan/ggml-rpc.dll', lock)
            for kept in ['runtime/text/cpu/llama-server.exe', 'runtime/text/cpu/llama-server-impl.dll',
                         'notices/whisper.cpp-LICENSE.txt']:
                self.assertEqual(lock[kept], old[kept])
            self.assertEqual((root / 'out/notices/whisper-cpu-cmake.txt').read_bytes(), b'cache cpu')
            self.assertIn('notices/whisper-vulkan-compiler.cmake', lock)
            self.assertIn('notices/native-runtime.cdx.json', lock)
            self.assertFalse((root / 'out/runtime/nemo').exists())
            self.assertNotIn('runtime/whisper/ggml-cpu.dll', lock)
            self.assertIn('runtime/whisper/ggml-vulkan.dll', lock)
            self.assertNotIn('runtime/whisper/cpu/ggml-vulkan.dll', lock)
            self.assertNotIn('runtime/whisper/vulkan-shaders-gen', lock)
            self.assertIn('notices/whisper-vulkan-build.json', lock)
            self.assertNotEqual(lock['runtime/whisper/whisper-server.exe'], old['runtime/whisper/whisper-server.exe'])

    def text_fixture(self, root, miniaudio=None):
        pin = json.loads((Path(__file__).resolve().parents[2] / 'scripts/inference/windows-text-source.json').read_text())
        for backend in ['cpu', 'vulkan']:
            build = root / f'text/text-{backend}'
            names = ['llama-server.exe', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu-x64.dll']
            names += ['ggml-vulkan.dll'] if backend == 'vulkan' else []
            files = {}
            for name in names:
                write(build / 'package' / name, pe(backend.encode() + name.encode()))
                files[name] = hashlib.sha256((build / 'package' / name).read_bytes()).hexdigest()
            write(build / 'build/CMakeCache.txt', b'cache')
            write(build / 'build-ggml/CMakeCache.txt', b'cache')
            write(build / 'source/LICENSE', b'MIT llama')
            write(build / 'source/vendor/miniaudio/miniaudio.h', miniaudio or b'x')
            (build / 'writer-build.json').write_text(json.dumps({'backend': backend, 'llama': pin['llama'],
                                                                  'miniaudio': pin['miniaudio'], 'files': files}))
        write(root / 'text/LICENSE-miniaudio', b'x')
        return ['--text-root', str(root / 'text'), '--miniaudio-license', str(root / 'text/LICENSE-miniaudio')]

    def test_text_runtime_requires_the_pinned_miniaudio(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            extra = self.text_fixture(root)
            result = subprocess.run([sys.executable, str(SCRIPT), '--base', str(root / 'base'), '--base-lock', str(root / 'base.lock.json'),
                                     '--build-root', str(root / 'build'), '--output', str(root / 'out'),
                                     '--lock-output', str(root / 'out.lock.json'), *extra], capture_output=True, text=True)
            self.assertIn('different miniaudio', result.stderr)
            self.assertFalse((root / 'out').exists())

    def test_refuses_legacy_worker_and_drifted_base(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root, server=b'legacy worker')
            self.assertIn('Legacy', self.run_script(root).stderr)
            self.assertFalse((root / 'out').exists())
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            base = self.fixture(root)
            (base / 'runtime/nemo/cpu/bin/nemo-speech.exe').write_bytes(pe(b'swapped'))
            self.assertIn('Package differs from lock', self.run_script(root).stderr)
            self.assertFalse((root / 'out').exists())


if __name__ == '__main__':
    unittest.main()
