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

# Exact counts, so a skipped or silently dropped test fails the run.
UI_TESTS = 33  # WriterUiTest 14 + ComplianceUiTest 6 + RenewalUiTest 13
STORAGE_CLASSES = ['WriterPresentationTest', 'SessionStoreTest', 'MobileConnectionTest', 'DirectCloudTest', 'CompanionTest', 'PhoneOfflineTest',
                   'AudioCaptureTest', 'CloudProviderTest', 'CustomTextModelsTest', 'WriterStateTest', 'SettingsSearchTest', 'FloatingStateTest',
                   'ProcessingStateTest', 'DeliveryStoreTest']
STORAGE_TESTS = 97


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
    # A fresh emulator has no Writer install yet; Android then reports an unknown package and
    # there is no app locale to restore.
    locales = re.search(r'\[(.*)\]', adb('shell', 'cmd', 'locale', 'get-app-locales', dev.PACKAGE).decode())
    original_locale = locales.group(1) if locales else ''
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
        # A full UI pass takes several minutes on a cold software-rendered emulator.
        output = dev.adb(args.serial, 'shell', 'am', 'instrument', '-w', '-e', 'class', classes, *extra, dev.PACKAGE + '.test/' + runner, timeout=1800)
        (private / (label + '.log')).write_bytes(output)
        if f'OK ({expected} tests)'.encode() not in output or b'FAILURES' in output:
            raise RuntimeError(f'{label} failed; inspect {private}')

    # MW-ANDROID-04: the renewal UI runs at 412 x 915 dp, at 150 % text with Hebrew RTL layout, and on a
    # 360 x 640 dp screen at 150 % text with RTL. Display overrides are restored in finally.
    ui_classes = ','.join('com.metrix.writer.' + name for name in ['WriterUiTest', 'ComplianceUiTest', 'RenewalUiTest'])
    passes = [('standard', '1.0', '0', None), ('large-rtl', '1.5', '1', None), ('small-large-rtl', '1.5', '1', ('1080x1920', '480'))]
    try:
        build(True)
        for label, scale, rtl, display in passes:
            adb('shell', 'settings', 'put', 'system', 'font_scale', scale)
            adb('shell', 'cmd', 'locale', 'set-app-locales', dev.PACKAGE, '--locales', 'he' if rtl == '1' else 'en')
            if display:
                adb('shell', 'wm', 'size', display[0]); adb('shell', 'wm', 'density', display[1])
            adb('shell', 'am', 'force-stop', dev.PACKAGE)
            try:
                instrument(label, ui_classes, 'androidx.test.runner.AndroidJUnitRunner', UI_TESTS, rtl)
            finally:
                if display:
                    adb('shell', 'wm', 'size', 'reset'); adb('shell', 'wm', 'density', 'reset')
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
    instrument('storage', ','.join('com.metrix.writer.' + name for name in STORAGE_CLASSES), 'android.test.InstrumentationTestRunner', STORAGE_TESTS)
    receipt = {'passed': True, 'serial': args.serial, 'ui_tests': UI_TESTS * len(passes), 'ui_passes': [p[0] for p in passes], 'storage_audio_engine_tests': STORAGE_TESTS,
               'apk_sha256': dev.installed_apk_sha256(args.serial), 'evidence': str(private),
               'physical_phone': False, 'live_cloud': False}
    (private / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    os.umask(0o077)
    main()
