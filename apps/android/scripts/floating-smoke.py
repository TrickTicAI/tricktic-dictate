#!/usr/bin/env python3
"""Delivery-only acceptance on an explicitly selected, owned Writer emulator."""
import argparse
import hashlib
import json
import shlex
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET

import dev


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    if not args.serial.startswith('emulator-'):
        raise ValueError('Floating fixtures must run on a Writer emulator, never a phone')
    private = Path(tempfile.mkdtemp(prefix='mw57-floating-', dir=dev.PRIVATE.parent))
    ime = dev.adb(args.serial, 'shell', 'settings', 'get', 'secure', 'default_input_method').strip()
    command = [dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument', '-w',
               '-e', 'class', 'com.metrix.writer.FloatingInputTest',
               dev.PACKAGE + '.test/android.test.InstrumentationTestRunner']
    # API 33 can retain a stale accessibility binding when instrumentation force-stops
    # an already-enabled target. Revoke only this owned debug fixture before starting;
    # preserve every other service. The suite grants and revokes via visible Settings.
    if dev.adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').strip() == b'33':
        enabled = dev.adb(args.serial, 'shell', 'settings', 'get', 'secure', 'enabled_accessibility_services').decode().strip()
        writer = {dev.PACKAGE + '/com.metrix.writer.WriterInputService', dev.PACKAGE + '/.WriterInputService'}
        services = [] if enabled in ('', 'null') else enabled.split(':')
        remaining = [service for service in services if service not in writer]
        if remaining != services:
            if remaining:
                dev.adb(args.serial, 'shell', 'settings', 'put', 'secure', 'enabled_accessibility_services', shlex.quote(':'.join(remaining)))
            else:
                dev.adb(args.serial, 'shell', 'settings', 'delete', 'secure', 'enabled_accessibility_services')
                dev.adb(args.serial, 'shell', 'settings', 'put', 'secure', 'accessibility_enabled', '0')
    # Its UiAutomation preserves accessibility services, unlike the ordinary uiautomator dump command.
    with (private / 'instrumentation.txt').open('wb') as output:
        subprocess.run(command, check=True, stdout=output, stderr=subprocess.STDOUT, timeout=540)
    result = (private / 'instrumentation.txt').read_text()
    if 'OK (21 tests)' not in result or 'FAILURES' in result:
        raise RuntimeError(f'Floating acceptance failed; inspect {private / "instrumentation.txt"}')
    assert ime == dev.adb(args.serial, 'shell', 'settings', 'get', 'secure', 'default_input_method').strip()

    def hashes():
        names = dev.adb(args.serial, 'shell', 'run-as', dev.PACKAGE, 'ls', 'files/deliveries').decode().splitlines()
        return {name: hashlib.sha256(dev.adb(args.serial, 'exec-out', 'run-as', dev.PACKAGE, 'cat',
                     'files/deliveries/' + name)).hexdigest() for name in names if name.endswith('.json')}

    before = hashes()
    assert before, 'No durable delivery evidence'
    dev.adb(args.serial, 'shell', 'am', 'force-stop', dev.PACKAGE)
    dev.adb(args.serial, 'shell', 'am', 'start', '-W', '-n', dev.PACKAGE + '/com.metrix.writer.FloatingSetupActivity')
    assert before == hashes(), 'Process recreation changed delivery results'
    # The suite ends with accessibility revoked, so this read-only UI dump cannot suppress an active Writer service.
    dev.adb(args.serial, 'shell', 'uiautomator', 'dump', '/sdcard/writer-floating-check.xml')
    tree = ET.fromstring(dev.adb(args.serial, 'shell', 'cat', '/sdcard/writer-floating-check.xml'))
    assert any('enable tricktic dictate · voice insertion' in n.get('text', '').casefold() for n in tree.iter('node'))
    apk = dev.ROOT / 'app/build/outputs/apk/dev/debug/app-dev-debug.apk'
    receipt = {
        'passed': True, 'delivery_only_fixture': True, 'tests': 21, 'serial': args.serial,
        'emulator_pid': owner['pid'], 'api': dev.adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').decode().strip(),
        'abi': dev.adb(args.serial, 'shell', 'getprop', 'ro.product.cpu.abi').decode().strip(),
        'apk_sha256': dev.installed_apk_sha256(args.serial),
        'keyboard_unchanged': True, 'delivery_files_unchanged_after_process_recreation': len(before),
        'real_transcription': False, 'physical_phone': False,
    }
    (private / 'result.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({**receipt, 'private_receipt': str(private)}, indent=2))


if __name__ == '__main__':
    main()
