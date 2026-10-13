#!/usr/bin/env python3
"""Actual APK phone inference; explicit owned emulator or authorized physical phone, private assets only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

import dev


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial', required=True)
    p.add_argument('--suite', type=Path, required=True, help='Private JSON: models [{id,path}], clips [{id,wav,mode,model,consented,provenance}]')
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--transfer-only', action='store_true', help='Owned emulator only: real HTTPS download pause/resume and rejected import')
    p.add_argument('--physical-authorized', action='store_true', help='Explicit consent to update debug Writer and test editor on the named phone')
    a = p.parse_args()
    dev.selected(a.serial)
    physical = dev.adb(a.serial, 'shell', 'getprop', 'ro.kernel.qemu').strip() != b'1'
    if physical and not a.physical_authorized: p.error('Physical tests require --physical-authorized')
    if not physical: dev.owned(a.serial)
    if physical and a.transfer_only: p.error('Transfer automation uses the owned emulator only')
    a.directory = a.directory.resolve()
    repo = dev.ROOT.parents[1]
    if a.directory.is_relative_to(repo): p.error('Keep private evidence outside source')
    a.directory.mkdir(parents=True, exist_ok=False)
    suite = json.loads(a.suite.read_text())
    catalog = json.loads((dev.ROOT / 'benchmark/assets.json').read_text())
    pinned = {m['id']: m for m in catalog['models']}
    config = {'physical_authorized': physical, 'models': [], 'clips': []}
    assets = []
    for m in suite['models']:
        path = Path(m['path']).resolve(); expected = pinned[m['id']]
        if path.stat().st_size != expected['bytes'] or hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() != expected['sha256']: p.error('Model hash/size mismatch')
        name = m['id'] + '.bin'; config['models'].append({'id': m['id'], 'file': name}); assets.append((path, name))
    for i, clip in enumerate(suite['clips']):
        if not clip.get('consented') or not clip.get('provenance'): p.error('Every speech clip requires explicit provenance and consent')
        path = Path(clip['wav']).resolve(); name = f'clip-{i}.wav'
        config['clips'].append({**{k: clip[k] for k in ['id', 'mode', 'model']}, 'file': name, 'sha256': hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()})
        assets.append((path, name))
    # Preserve the phone's actual dictionary/engine choices around the instrumentation fixture.
    settings_backup = {}
    for name in ['companion.xml', 'phone-engine.xml']:
        result = subprocess.run([dev.tool('platform-tools/adb'), '-s', a.serial, 'exec-out', 'run-as', dev.PACKAGE, 'cat', 'shared_prefs/' + name], capture_output=True)
        settings_backup[name] = result.stdout if result.returncode == 0 and result.stdout.startswith(b'<?xml') else None
        if settings_backup[name] is not None: (a.directory / name).write_bytes(settings_backup[name])
    for name in ['app/build/outputs/apk/dev/debug/app-dev-debug.apk', 'app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk', 'editor/build/outputs/apk/debug/editor-debug.apk']:
        dev.adb(a.serial, 'install', '-r', str(dev.ROOT / name))
    appdir = 'no_backup/phone-proof'
    dev.adb(a.serial, 'shell', 'run-as', dev.PACKAGE, 'mkdir', '-p', appdir)
    (a.directory / 'config.json').write_text(json.dumps(config))
    assets.append((a.directory / 'config.json', 'config.json'))
    for source, name in assets:
        with source.open('rb') as data:
            subprocess.run([dev.tool('platform-tools/adb'), '-s', a.serial, 'shell', 'run-as', dev.PACKAGE, 'sh', '-c', f'"cat > {appdir}/{name}"'], stdin=data, stdout=subprocess.DEVNULL, check=True)
    # Only the owned emulator is automatically put offline. A phone must already be offline.
    if a.transfer_only:
        dev.adb(a.serial, 'shell', 'cmd', 'connectivity', 'airplane-mode', 'disable')
        dev.adb(a.serial, 'shell', 'svc', 'wifi', 'enable')
    elif not physical:
        dev.adb(a.serial, 'shell', 'cmd', 'connectivity', 'airplane-mode', 'enable')
        dev.adb(a.serial, 'shell', 'svc', 'wifi', 'disable')
        dev.adb(a.serial, 'shell', 'svc', 'data', 'disable')
    for setting, expected in ([] if a.transfer_only else [('airplane_mode_on', b'1'), ('wifi_on', b'0')]):
        if dev.adb(a.serial, 'shell', 'settings', 'get', 'global', setting).strip() != expected: p.error('Disable phone networking first (airplane mode on, Wi-Fi off)')
    enabled = dev.adb(a.serial, 'shell', 'settings', 'get', 'secure', 'enabled_accessibility_services').decode().strip()
    services = [s for s in enabled.split(':') if s and s != 'null']
    component = dev.PACKAGE + '/com.metrix.writer.WriterInputService'
    try:
        dev.adb(a.serial, 'shell', 'pm', 'grant', dev.PACKAGE, 'android.permission.RECORD_AUDIO')
        dev.adb(a.serial, 'shell', 'pm', 'grant', dev.PACKAGE, 'android.permission.POST_NOTIFICATIONS')
        dev.adb(a.serial, 'shell', 'appops', 'set', dev.PACKAGE, 'SYSTEM_ALERT_WINDOW', 'allow')
        dev.adb(a.serial, 'shell', 'settings', 'put', 'secure', 'enabled_accessibility_services', ':'.join(dict.fromkeys(services + [component])))
        for phase in (['transfer'] if a.transfer_only else ['run', 'resume']):
            with (a.directory / f'{phase}.log').open('wb') as out:
                subprocess.run([dev.tool('platform-tools/adb'), '-s', a.serial, 'shell', 'am', 'instrument', '-w', '-e', 'phase', phase,
                                dev.PACKAGE + '.test/com.metrix.writer.PhoneDictationRunner'], stdout=out, stderr=subprocess.STDOUT, check=True, timeout=1500)
            for suffix in ['results.json', 'failure.json']:
                result = subprocess.run([dev.tool('platform-tools/adb'), '-s', a.serial, 'exec-out', 'run-as', dev.PACKAGE, 'cat', f'{appdir}/{phase}-{suffix}'], capture_output=True)
                if result.returncode == 0 and result.stdout.startswith(b'{'): (a.directory / f'{phase}-{suffix}').write_bytes(result.stdout)
            if f'PHONE_{phase.upper()}_PASSED' not in (a.directory / f'{phase}.log').read_text(): raise RuntimeError(f'{phase} failed; inspect private evidence at {a.directory}')
            dev.adb(a.serial, 'shell', 'am', 'force-stop', dev.PACKAGE)
        receipt = {'passed': True, 'physical': physical, 'serial': a.serial, 'runtime': catalog['runtime'],
                   'apk_sha256': dev.installed_apk_sha256(a.serial),
                   'device': dev.adb(a.serial, 'shell', 'getprop', 'ro.build.fingerprint').decode().strip(),
                   'model_transfer_only': a.transfer_only, 'speech_source': 'explicit saved speech at test-only AudioCapture source boundary', 'microphone_quality': False}
    finally:
        dev.adb(a.serial, 'shell', 'am', 'force-stop', dev.PACKAGE)
        for name, data in settings_backup.items():
            if data is None:
                dev.adb(a.serial, 'shell', 'run-as', dev.PACKAGE, 'rm', '-f', 'shared_prefs/' + name)
            else:
                subprocess.run([dev.tool('platform-tools/adb'), '-s', a.serial, 'shell', 'run-as', dev.PACKAGE, 'sh', '-c', f'"cat > shared_prefs/{name}"'], input=data, stdout=subprocess.DEVNULL, check=True)
        if enabled and enabled != 'null':
            dev.adb(a.serial, 'shell', 'settings', 'put', 'secure', 'enabled_accessibility_services', enabled)
        else:
            dev.adb(a.serial, 'shell', 'settings', 'delete', 'secure', 'enabled_accessibility_services')
        restored = dev.adb(a.serial, 'shell', 'settings', 'get', 'secure', 'enabled_accessibility_services').decode().strip()
        if (restored if restored != 'null' else '') != (enabled if enabled != 'null' else ''):
            raise RuntimeError('Accessibility setting restoration did not match; inspect the private receipt before continuing')
    # Successful inference is not a successful test run until cleanup also succeeds.
    receipt['settings_restored'] = True
    (a.directory / 'receipt.json').write_text(json.dumps(receipt, indent=2))
    print(json.dumps({'passed': True, 'physical': physical, 'evidence': str(a.directory)}))


if __name__ == '__main__':
    os.umask(0o077)
    main()
