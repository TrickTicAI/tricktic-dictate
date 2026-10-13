#!/usr/bin/env python3
"""Compose UI, statistics and recovery checks on an explicitly owned Writer emulator."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile

import dev


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    dev.selected(args.serial)
    dev.owned(args.serial)
    if not args.serial.startswith('emulator-'):
        parser.error('Presentation fixtures run only on owned emulators')
    private = Path(tempfile.mkdtemp(prefix='writer-presentation-', dir=dev.PRIVATE.parent))
    adb = lambda *parts: dev.adb(args.serial, *parts)
    original_font = adb('shell', 'settings', 'get', 'system', 'font_scale').decode().strip()
    original_locale = re.search(r'\[(.*)\]', adb('shell', 'cmd', 'locale', 'get-app-locales', dev.PACKAGE).decode()).group(1)
    app = dev.ROOT / dev.APP_APK
    test = dev.ROOT / dev.TEST_APK

    def build(ui):
        command = [str(dev.ROOT / 'gradlew'), '-p', str(dev.ROOT), *dev.DEBUG_TASKS]
        if ui:
            command.append('-PwriterTestRunner=androidx.test.runner.AndroidJUnitRunner')
        with (private / ('ui-build.log' if ui else 'storage-build.log')).open('wb') as out:
            subprocess.run(command, check=True, stdout=out, stderr=subprocess.STDOUT)
        adb('install', '-r', str(app)); adb('install', '-r', str(test))

    def instrument(label, classes, runner, expected, rtl=None):
        extra = ['-e', 'writerExpectedRtl', rtl] if rtl is not None else []
        output = adb('shell', 'am', 'instrument', '-w', '-e', 'class', classes, *extra, dev.PACKAGE + '.test/' + runner)
        (private / (label + '.log')).write_bytes(output)
        if f'OK ({expected} tests)'.encode() not in output or b'FAILURES' in output:
            raise RuntimeError(f'{label} failed; inspect {private}')

    try:
        build(True)
        for label, scale, rtl in [('standard', '1.0', '0'), ('large-rtl', '1.5', '1')]:
            adb('shell', 'settings', 'put', 'system', 'font_scale', scale)
            adb('shell', 'cmd', 'locale', 'set-app-locales', dev.PACKAGE, '--locales', 'he' if rtl == '1' else 'en')
            adb('shell', 'am', 'force-stop', dev.PACKAGE)
            instrument(label, 'com.metrix.writer.WriterUiTest', 'androidx.test.runner.AndroidJUnitRunner', 14, rtl)
        names = adb('shell', 'run-as', dev.PACKAGE, 'ls', 'cache/ui-proof').decode().splitlines()
        for name in names:
            if Path(name).name != name or not name.endswith('.png'):
                raise RuntimeError('Unexpected screenshot filename')
            (private / name).write_bytes(adb('exec-out', 'run-as', dev.PACKAGE, 'cat', 'cache/ui-proof/' + name))
    finally:
        for table, key, value in [('system', 'font_scale', original_font)]:
            adb('shell', 'settings', 'delete', table, key) if value == 'null' else adb('shell', 'settings', 'put', table, key, value)
        adb('shell', 'cmd', 'locale', 'set-app-locales', dev.PACKAGE, *(['--locales', original_locale] if original_locale else []))
        # Other device smoke tools use the framework runner; always restore its APK.
        build(False)
    instrument('storage', ','.join('com.metrix.writer.' + name for name in ['WriterPresentationTest', 'SessionStoreTest', 'MobileConnectionTest', 'DirectCloudTest', 'CompanionTest', 'PhoneOfflineTest', 'AudioCaptureTest', 'CloudProviderTest', 'CustomTextModelsTest', 'WriterStateTest', 'SettingsSearchTest']), 'android.test.InstrumentationTestRunner', 76)
    receipt = {'passed': True, 'serial': args.serial, 'ui_tests': 28, 'storage_audio_engine_tests': 76,
               'apk_sha256': dev.installed_apk_sha256(args.serial), 'evidence': str(private),
               'physical_phone': False, 'live_cloud': False}
    (private / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    os.umask(0o077)
    main()
