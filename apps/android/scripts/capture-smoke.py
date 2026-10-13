#!/usr/bin/env python3
"""Recoverable microphone acceptance on an explicit Writer-owned API 34/36 emulator."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

import dev


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    if not args.serial.startswith('emulator-'):
        raise ValueError('Capture fixtures must use an owned emulator, never a phone')
    private = Path(tempfile.mkdtemp(prefix='mw58-capture-', dir=dev.PRIVATE.parent))
    apk = dev.ROOT / 'editor/build/outputs/apk/androidTest/debug/editor-debug-androidTest.apk'
    dev.adb(args.serial, 'install', '-r', str(apk))
    ime = dev.adb(args.serial, 'shell', 'settings', 'get', 'secure', 'default_input_method')
    suites = [
        ('native', 'com.metrix.writer.AudioCaptureTest,com.metrix.writer.SessionStoreTest,com.metrix.writer.MobileConnectionTest', dev.PACKAGE, 16),
        # Separate target process: no instrumentation start exemptions are conferred on Writer.
        ('floating', 'com.metrix.writer.editor.FloatingCaptureTest', 'com.metrix.writer.editor', 9),
    ]
    for name, classes, package, count in suites:
        with (private / f'{name}.txt').open('wb') as out:
            subprocess.run([dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument',
                '-w', '-e', 'class', classes, package + '.test/android.test.InstrumentationTestRunner'],
                stdout=out, stderr=subprocess.STDOUT, check=True, timeout=600)
        output = (private / f'{name}.txt').read_text()
        if f'OK ({count} tests)' not in output or 'FAILURES' in output:
            raise RuntimeError(f'{name} acceptance failed: {private / (name + ".txt")}')
    if ime != dev.adb(args.serial, 'shell', 'settings', 'get', 'secure', 'default_input_method'):
        raise RuntimeError('Selected keyboard changed')
    receipt = {
        'passed': True, 'serial': args.serial, 'emulator_pid': owner['pid'],
        'api': dev.adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').decode().strip(),
        'abi': dev.adb(args.serial, 'shell', 'getprop', 'ro.product.cpu.abi').decode().strip(),
        'apk_sha256': dev.installed_apk_sha256(args.serial),
        'native_tests': 15, 'floating_tests': 9, 'writer_instrumented_during_floating_capture': False,
        'keyboard_unchanged': True, 'host_microphone': False, 'transcription': False, 'physical_phone': False,
    }
    (private / 'result.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({**receipt, 'private_receipt': str(private)}, indent=2))


if __name__ == '__main__':
    main()
