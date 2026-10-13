#!/usr/bin/env python3
"""Build the pinned staged NeMo worker with a pinned static SentencePiece dependency."""
import argparse
import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags, roots, write_receipt  # noqa: E402

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--sentencepiece-source', type=Path, required=True)
p.add_argument('--sentencepiece-build', type=Path, required=True)
p.add_argument('--backend', choices=['metal', 'cpu'], required=True)
a = p.parse_args()
if platform.system() != 'Darwin' or platform.machine() != 'arm64':
    p.error('Build on Apple Silicon')
source, output = a.source.resolve(), a.output.resolve()
if not (source / 'writer-source.txt').is_file():
    p.error('Stage NeMo with stage-nemo.py first')
sp = a.sentencepiece_source.resolve()
revision = subprocess.check_output(['git', '-C', str(sp), 'rev-parse', 'HEAD'], text=True).strip()
if revision != '17d7580d6407802f85855d2cc9190634e2c95624' or subprocess.check_output(['git', '-C', str(sp), 'status', '--porcelain']):
    p.error('SentencePiece must be the clean upstream pin')
sp_build = a.sentencepiece_build.resolve()
# No build-machine paths in __FILE__ strings (MW-RELEASE-14); SentencePiece links statically.
mapped = ' '.join(prefix_flags(roots(nemo_source=source, nemo_build=output, sentencepiece_source=sp, sentencepiece_build=sp_build)))
sp_command = ['cmake', '-S', str(sp), '-B', str(sp_build), '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
    '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_OSX_ARCHITECTURES=arm64', '-DCMAKE_OSX_DEPLOYMENT_TARGET=15.0',
    '-DSPM_BUILD_TEST=OFF', '-DSPM_ENABLE_SHARED=OFF', '-DSPM_ENABLE_TCMALLOC=OFF',
    f'-DCMAKE_C_FLAGS={mapped}', f'-DCMAKE_CXX_FLAGS={mapped}']
subprocess.run(sp_command, check=True)
subprocess.run(['cmake', '--build', str(sp_build), '--target', 'sentencepiece-static', '--parallel', '4'], check=True)
archive = sp_build / 'src/libsentencepiece.a'
settings = {
    'CMAKE_BUILD_TYPE': 'Release', 'CMAKE_OSX_ARCHITECTURES': 'arm64',
    'CMAKE_OSX_DEPLOYMENT_TARGET': '15.0', 'CMAKE_INSTALL_PREFIX': str(output / 'package'),
    'CMAKE_BUILD_WITH_INSTALL_RPATH': 'ON', 'GGML_NATIVE': 'OFF',
    'GGML_METAL': 'ON' if a.backend == 'metal' else 'OFF', 'GGML_METAL_EMBED_LIBRARY': 'ON',
    'GGML_ACCELERATE': 'ON', 'GGML_BLAS': 'ON', 'GGML_BLAS_VENDOR': 'Apple',
    'GGML_CUDA': 'OFF', 'GGML_VULKAN': 'OFF', 'NEMO_SPEECH_BUILD_TTS': 'OFF',
    'NEMO_SPEECH_BUILD_NMT': 'OFF', 'NEMO_SPEECH_BUILD_DIAR': 'OFF',
    'NEMO_SPEECH_BUILD_MIC_CAPTURE': 'OFF', 'NEMO_SPEECH_BUILD_HTTP': 'ON',
    'NEMO_SPEECH_HTTP_TLS': 'OFF', 'NEMO_SPEECH_BUILD_TESTS': 'OFF',
    'NEMO_SPEECH_BUILD_EXAMPLES': 'OFF', 'NEMO_SPEECH_GGML_PATCHED': 'OFF', 'SENTENCEPIECE_LIB': str(archive),
    'SENTENCEPIECE_INCLUDE_DIR': str(sp / 'src'),
    'CMAKE_C_FLAGS': mapped, 'CMAKE_CXX_FLAGS': mapped, 'CMAKE_OBJC_FLAGS': mapped,
}
command = ['cmake', '-S', str(source), '-B', str(output)] + [f'-D{k}={v}' for k,v in settings.items()]
subprocess.run(command, check=True)
subprocess.run(['cmake', '--build', str(output), '--parallel', '4'], check=True)
subprocess.run(['cmake', '--install', str(output)], check=True)
receipt = {'source': (source / 'writer-source.txt').read_text(), 'backend': a.backend,
    'configure': command, 'compiler': subprocess.check_output(['cc', '--version'], text=True),
    'sentencepiece': {'revision': revision, 'configure': sp_command, 'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()},
    'files': {str(f.relative_to(output / 'package')): hashlib.sha256(f.read_bytes()).hexdigest()
        for f in sorted((output / 'package').rglob('*')) if f.is_file()}}
write_receipt(output / 'writer-build.json', receipt)
