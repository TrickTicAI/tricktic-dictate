#!/usr/bin/env python3
"""Prove release upgrade retention on an empty Writer-owned emulator (never a phone)."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

import dev
import release

PACKAGE = 'com.metrix.writer'
COMPONENT = PACKAGE + '/com.metrix.writer.WriterInputService'


def main():
    os.umask(0o077)
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial', required=True)
    p.add_argument('--previous', type=Path, required=True)
    p.add_argument('--current', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    if not args.serial.startswith('emulator-'):
        raise ValueError('Upgrade fixtures must never seed a physical phone')
    dev.selected(args.serial); owner = dev.owned(args.serial)
    output = release.external(args.output); output.mkdir(parents=True, exist_ok=False)
    # Historical APKs may contain the now-removed dormant delivery fixture. The new candidate may not.
    before = release.inspect(args.previous / 'writer.apk', allow_legacy_fixture=True)
    after = release.inspect(args.current / 'writer.apk')
    if before['signer_sha256'] != after['signer_sha256'] or before['version_code'] >= after['version_code']:
        raise ValueError('Need an increasing version code and the same signer')
    if ('package:' + PACKAGE) in dev.adb(args.serial, 'shell', 'pm', 'list', 'packages', PACKAGE).decode().splitlines():
        raise ValueError('Release package already installed; use a fresh isolated emulator without private data')
    def shell(*items): return dev.adb(args.serial, 'shell', *items).decode().strip()
    keyboard = shell('settings', 'get', 'secure', 'default_input_method')
    services = shell('settings', 'get', 'secure', 'enabled_accessibility_services')
    services = '' if services == 'null' else services
    def install(directory):
        for name in ('writer.apk', 'upgrade-test.apk'):
            dev.adb(args.serial, 'install', '-r', str(directory / name))
    def phase(name, version):
        command = [dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument', '-w',
                   '-e', 'phase', name, '-e', 'version', str(version), PACKAGE + '.test/com.metrix.writer.UpgradeRunner']
        result = subprocess.run(command, check=True, capture_output=True, timeout=120).stdout.decode()
        (output / (name + '.txt')).write_text(result)
        if 'result=PASS' not in result or 'INSTRUMENTATION_CODE: 0' not in result:
            raise RuntimeError('Upgrade phase failed; inspect ' + str(output / (name + '.txt')))
    def grants():
        dump = shell('dumpsys', 'package', PACKAGE)
        return {permission: bool(re.search(re.escape(permission) + r': granted=true', dump)) for permission in
                ('android.permission.RECORD_AUDIO', 'android.permission.POST_NOTIFICATIONS')} | {
            'overlay': 'allow' in shell('appops', 'get', PACKAGE, 'SYSTEM_ALERT_WINDOW'),
            'accessibility': COMPONENT in shell('settings', 'get', 'secure', 'enabled_accessibility_services')}
    def launch():
        shell('am', 'start', '-W', '-n', PACKAGE + '/com.metrix.writer.FloatingSetupActivity')
        time.sleep(2)
    def reconnect():
        # Instrumentation/process death can leave an enabled but dead service binding.
        shell('settings', 'put', 'secure', 'enabled_accessibility_services', services or 'null')
        time.sleep(1)
        shell('settings', 'put', 'secure', 'enabled_accessibility_services', ':'.join(filter(None, [services, COMPONENT])))
        shell('settings', 'put', 'secure', 'accessibility_enabled', '1')
        for _ in range(30):
            state = shell('dumpsys', 'activity', 'services', PACKAGE)
            active = state.split('Connection bindings to services:')[0]
            if 'ServiceRecord{' in active and 'WriterInputService' in active and 'hasBound=true' in active:
                return
            time.sleep(0.2)
        raise RuntimeError('Writer editor service did not reconnect')
    def idle():
        service = shell('dumpsys', 'activity', 'services', PACKAGE)
        if '.CaptureService' in service:
            raise RuntimeError('Capture service started without a user recording tap')
    install(args.previous)
    launch()
    # Emulator setup only; physical Samsung permission actions must be performed by a person on the device.
    for permission in ('android.permission.RECORD_AUDIO', 'android.permission.POST_NOTIFICATIONS'):
        shell('pm', 'grant', PACKAGE, permission)
    shell('appops', 'set', PACKAGE, 'SYSTEM_ALERT_WINDOW', 'allow')
    shell('settings', 'put', 'secure', 'enabled_accessibility_services', ':'.join(filter(None, [services, COMPONENT])))
    shell('settings', 'put', 'secure', 'accessibility_enabled', '1')
    time.sleep(2)
    phase('seed', before['version_code'])
    initial = grants(); assert all(initial.values()), 'Baseline grants missing'
    install(args.current)
    # Check grants before launching the app so a recovery step cannot mask OS revocation.
    upgraded = grants(); assert upgraded == initial, 'Upgrade changed grants'
    phase('verify', after['version_code']); launch(); idle()
    # Physical reboot and Samsung sleep are separate. This is an actual emulator reboot.
    dev.adb(args.serial, 'reboot')
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            if shell('getprop', 'sys.boot_completed') == '1': break
        except subprocess.CalledProcessError: pass
        time.sleep(2)
    else: raise TimeoutError('Owned emulator failed to reboot')
    shell('input', 'keyevent', '82')
    restored = grants(); assert restored == initial, 'Reboot changed grants'
    idle(); phase('reboot', after['version_code']); launch(); idle()
    # Permission revocation/regrant never resumes capture. Existing saved bytes still match.
    shell('pm', 'revoke', PACKAGE, 'android.permission.RECORD_AUDIO')
    launch(); idle()
    shell('pm', 'grant', PACKAGE, 'android.permission.RECORD_AUDIO')
    launch(); idle(); phase('regrant', after['version_code'])
    reconnect(); idle()
    assert keyboard == shell('settings', 'get', 'secure', 'default_input_method')
    dev.adb(args.serial, 'uninstall', PACKAGE + '.test')  # Leave the actual release installed.
    receipt = {'passed': True, 'previous': before, 'current': after, 'serial': args.serial,
               'emulator_pid': owner['pid'], 'grants_before': initial, 'grants_after': upgraded,
               'grants_after_reboot': restored, 'keyboard_unchanged': True,
               'seed': 'synthetic audio/text; fake legacy and direct Gemini credentials on isolated emulator',
               'reboot_and_microphone_regrant_retained_data': True, 'silent_microphone_restart': False,
               'editor_service_reconnected_after_test_process_exit': True,
               'physical_phone': False, 'real_transcription': False}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__': main()
