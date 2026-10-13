#!/usr/bin/env python3
"""Export legacy debug sessions or import their unchanged WAV/sidecars into Writer debug."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile
import uuid
from dev import adb, selected, PACKAGE, run, tool

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('command', choices=['export', 'import'])
p.add_argument('--serial', required=True)
p.add_argument('--archive', type=Path, required=True)
p.add_argument('--source-package', choices=['com.metrix.writer.spike', PACKAGE], default='com.metrix.writer.spike')
a = p.parse_args()
selected(a.serial)
if a.command == 'export':
    # Explicit export stops only the selected Writer app, keeping a coherent byte snapshot.
    adb(a.serial, 'shell', 'am', 'force-stop', a.source_package)
    data = adb(a.serial, 'exec-out', 'run-as', a.source_package, 'tar', '-cf', '-', 'files/recordings')
    with a.archive.open('xb') as out: out.write(data)
    a.archive.chmod(0o600)
    print(json.dumps({'archive': str(a.archive), 'sha256': hashlib.sha256(data).hexdigest()}))
else:
    files = {}
    with tarfile.open(a.archive) as archive:
        for entry in archive:
            if entry.isdir(): continue
            path = Path(entry.name)
            if not entry.isfile() or path.parent.as_posix() != 'files/recordings' or path.suffix not in ('.json', '.wav'):
                raise ValueError('Archive contains an unsupported path; restore/repair the old app before export')
            if str(uuid.UUID(path.stem)) != path.stem or path.name in files: raise ValueError('Invalid or duplicate session')
            files[path.name] = archive.extractfile(entry).read()
    for name, data in files.items():
        if name.endswith('.json'):
            item = json.loads(data)
            if name != item['id'] + '.json' or item['id'] + '.wav' not in files: raise ValueError('Missing matching WAV')
    if not files or any(name[:-4] + '.json' not in files for name in files if name.endswith('.wav')):
        raise ValueError('Empty/incomplete export')
    adb(a.serial, 'shell', 'am', 'force-stop', PACKAGE)
    adb(a.serial, 'shell', 'run-as', PACKAGE, 'mkdir', '-p', 'files/recordings')
    existing = adb(a.serial, 'shell', 'run-as', PACKAGE, 'ls', 'files/recordings').decode().split()
    if set(files).intersection(existing): raise ValueError('Refusing to overwrite an existing session')
    # Stage the validated flat archive; target code never reads partial JSON.
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w') as archive:
        for name, data in files.items():
            info = tarfile.TarInfo(name if name.endswith('.wav') else name + '.new')
            info.size = len(data); info.mode = 0o600
            archive.addfile(info, io.BytesIO(data))
    run([tool('platform-tools/adb'), '-s', a.serial, 'exec-in', 'run-as', PACKAGE,
         'tar', '-xf', '-', '-C', 'files/recordings'], input=buffer.getvalue())
    for name, data in files.items():
        staged = name if name.endswith('.wav') else name + '.new'
        actual = adb(a.serial, 'exec-out', 'run-as', PACKAGE, 'cat', 'files/recordings/' + staged)
        if actual != data: raise ValueError('Imported bytes differ')
    for name in files:
        if name.endswith('.json'):
            adb(a.serial, 'shell', 'run-as', PACKAGE, 'mv', 'files/recordings/' + name + '.new', 'files/recordings/' + name)
    print(json.dumps({'sessions_imported': sum(n.endswith('.json') for n in files), 'all_bytes_verified': True}))
