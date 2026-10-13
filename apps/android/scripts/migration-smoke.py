#!/usr/bin/env python3
"""Byte-level migration proof using an explicitly supplied old spike APK on a Writer emulator."""
import argparse
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import uuid
from pathlib import Path
from dev import adb, selected, tool, run, ROOT, PACKAGE

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--serial', required=True)
p.add_argument('--legacy-apk', type=Path, required=True)
a = p.parse_args()
selected(a.serial)
if not a.serial.startswith('emulator-') or not adb(a.serial, 'emu', 'avd', 'name').startswith(b'Writer_'):
    raise ValueError('Use a Writer-owned emulator')
legacy = 'com.metrix.writer.spike'
installed = adb(a.serial, 'shell', 'pm', 'list', 'packages', legacy).decode().splitlines()
if 'package:' + legacy in installed: raise ValueError('Refusing to replace an existing spike installation')
adb(a.serial, 'install', str(a.legacy_apk.resolve()))
adb(a.serial, 'shell', 'run-as', legacy, 'mkdir', '-p', 'files/recordings')
id = str(uuid.uuid4())
# Non-speech synthetic WAV with a complete header, previous raw result and an uncertain request.
import struct
pcm = bytes(range(128)) * 250
wav = b'RIFF' + struct.pack('<I', len(pcm) + 36) + b'WAVEfmt ' + struct.pack('<IHHIIHH', 16, 1, 1, 16000, 32000, 2, 16) + b'data' + struct.pack('<I', len(pcm)) + pcm
item = {'id': id, 'created_at_ms': 1, 'mime_type': 'audio/wav', 'byte_length': len(wav), 'duration_ms': 1000, 'input_language': 'mixed', 'capture_state': 'complete',
        'attempts': [{'id': str(uuid.uuid4()), 'status': {'state': 'succeeded', 'result': {'transcript': {'raw_text': 'Migration fixture עברית English'}}}}],
        'pending_server': 'https://example.invalid', 'pending': {'recording_id': id, 'idempotency_key': str(uuid.uuid4()), 'options': {'selection': {'provider': 'local', 'model': 'whisper-tiny'}, 'input_language': 'mixed'}}, 'extension_from_spike': {'preserve': True}}
metadata = json.dumps(item, ensure_ascii=False).encode()
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode='w') as archive:
    for suffix, data in (('.wav', wav), ('.json', metadata)):
        entry = tarfile.TarInfo(id + suffix); entry.size = len(data); entry.mode = 0o600
        archive.addfile(entry, io.BytesIO(data))
run([tool('platform-tools/adb'), '-s', a.serial, 'exec-in', 'run-as', legacy, 'tar', '-xf', '-', '-C', 'files/recordings'], input=buffer.getvalue())
private = Path(tempfile.mkdtemp(prefix='writer-migration-'))
archive = private / 'legacy.tar'
subprocess.run([sys.executable, str(ROOT / 'scripts/transfer.py'), 'export', '--serial', a.serial, '--archive', str(archive)], check=True)
subprocess.run([sys.executable, str(ROOT / 'scripts/transfer.py'), 'import', '--serial', a.serial, '--archive', str(archive)], check=True)
for suffix, expected in (('.wav', wav), ('.json', metadata)):
    for package in (legacy, PACKAGE):
        assert adb(a.serial, 'exec-out', 'run-as', package, 'cat', 'files/recordings/' + id + suffix) == expected
# Overwrite refusal must leave both app copies identical.
refused = subprocess.run([sys.executable, str(ROOT / 'scripts/transfer.py'), 'import', '--serial', a.serial, '--archive', str(archive)], capture_output=True)
assert refused.returncode != 0 and b'Refusing to overwrite' in refused.stderr
print(json.dumps({'passed': True, 'serial': a.serial, 'sessions': 1, 'wav_and_metadata_byte_identical': True, 'source_retained': True, 'overwrite_refused': True, 'archive': str(archive)}))
