#!/usr/bin/env python3
"""Google Play phone screenshots from the play build on an owned Writer emulator.

capture: builds the play debug APKs, reinstalls them fresh, pins the clock, status
bar and a 1080x1920 (9:16) display, runs StoreScreenshots with invented fixtures,
and writes 24-bit PNGs without alpha. Device settings are restored afterwards.
compare: checks that two capture runs produced equivalent screenshots.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

os.environ['WRITER_ANDROID_FLAVOR'] = 'play'  # Listing screenshots always show the Play app.
import dev

from PIL import Image, ImageChops

REPO = dev.ROOT.parents[1]
DEFAULT_OUTPUT = REPO / 'docs/release/store-listing/assets/screenshots'
SIZE = (1080, 1920)
SCREENS = ['01-home', '02-history', '03-spoken-and-polished', '04-dictionary',
           '05-floating-control', '06-dictation-engine', '07-statistics-ink']
NOW_MS = 1_790_184_600_000  # 2026-09-23 17:30 UTC; must equal StoreScreenshots.NOW.
RUNNER = 'androidx.test.runner.AndroidJUnitRunner'
ANIMATIONS = ['window_animation_scale', 'transition_animation_scale', 'animator_duration_scale']


def build():
    command = [str(dev.ROOT / 'gradlew'), '-p', str(dev.ROOT), *dev.DEBUG_TASKS, f'-PwriterTestRunner={RUNNER}']
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode:
        sys.stderr.write(result.stdout[-4000:] + result.stderr[-4000:])
        raise SystemExit('Play debug build failed')


def flatten(png: bytes) -> Image.Image:
    image = Image.open(io.BytesIO(png))
    image.load()
    if image.size != SIZE:
        raise RuntimeError(f'Expected a {SIZE[0]}x{SIZE[1]} screenshot, got {image.size[0]}x{image.size[1]}')
    if image.mode == 'RGBA' and image.getchannel('A').getextrema() != (255, 255):
        raise RuntimeError('Screenshot has transparent pixels')
    return image.convert('RGB')


def capture(serial: str, output: Path, skip_build: bool):
    dev.selected(serial)
    dev.owned(serial)
    if not serial.startswith('emulator-'):
        raise SystemExit('Listing screenshots run only on owned emulators')
    adb = lambda *parts: dev.adb(serial, *parts).decode().strip()
    shell = lambda *parts: adb('shell', *parts)
    if not skip_build:
        build()
    private = Path(tempfile.mkdtemp(prefix='writer-store-screenshots-', dir=dev.PRIVATE))

    original = {'global': {key: shell('settings', 'get', 'global', key) for key in [*ANIMATIONS, 'auto_time', 'auto_time_zone', 'sysui_demo_allowed']},
                'system': {'font_scale': shell('settings', 'get', 'system', 'font_scale')},
                'timezone': shell('getprop', 'persist.sys.timezone'),
                'night': shell('cmd', 'uimode', 'night').rsplit(':', 1)[-1].strip()}
    demo = lambda command, *extra: shell('am', 'broadcast', '-a', 'com.android.systemui.demo', '-e', 'command', command, *extra)
    try:
        # A fresh install: the fixtures must be the only data on screen.
        for package in (dev.PACKAGE + '.test', dev.PACKAGE):
            if shell('pm', 'list', 'packages', package).splitlines().count('package:' + package):
                adb('uninstall', package)
        adb('install', str(dev.ROOT / dev.APP_APK)); adb('install', str(dev.ROOT / dev.TEST_APK))
        # An emulator killed without a disk flush can keep a stale data directory across reinstall.
        if shell('pm', 'clear', dev.PACKAGE) != 'Success':
            raise RuntimeError('Could not clear the app data')
        shell('cmd', 'locale', 'set-app-locales', dev.PACKAGE, '--locales', 'en-US')
        for key in ANIMATIONS:
            shell('settings', 'put', 'global', key, '0')
        shell('settings', 'put', 'system', 'font_scale', '1.0')
        shell('cmd', 'uimode', 'night', 'no')
        shell('settings', 'put', 'global', 'auto_time', '0')
        shell('settings', 'put', 'global', 'auto_time_zone', '0')
        shell('cmd', 'alarm', 'set-timezone', 'UTC')
        shell('cmd', 'alarm', 'set-time', str(NOW_MS))
        shell('wm', 'size', f'{SIZE[0]}x{SIZE[1]}')
        shell('settings', 'put', 'global', 'sysui_demo_allowed', '1')
        # The first demo session after boot shows a second Wi-Fi icon; a second session does not.
        for _ in range(2):
            demo('exit')
            time.sleep(1)
            demo('enter')
            demo('clock', '-e', 'hhmm', '1730')
            demo('battery', '-e', 'level', '100', '-e', 'plugged', 'false', '-e', 'powersave', 'false')
            demo('network', '-e', 'wifi', 'show', '-e', 'level', '4', '-e', 'fully', 'true')
            demo('network', '-e', 'mobile', 'hide')
            demo('notifications', '-e', 'visible', 'false')
        result = subprocess.run([dev.tool('platform-tools/adb'), '-s', serial, 'shell', 'am', 'instrument', '-w', '-e', 'writerStoreScreenshots', '1',
                                 '-e', 'class', 'com.metrix.writer.StoreScreenshots', f'{dev.PACKAGE}.test/{RUNNER}'],
                                check=True, capture_output=True, text=True, timeout=600).stdout
        (private / 'instrumentation.log').write_text(result + '\n')
        if 'OK (1 test)' not in result or 'FAILURES' in result:
            raise RuntimeError(f'StoreScreenshots failed; inspect {private}')
        names = shell('run-as', dev.PACKAGE, 'ls', 'cache/store-screenshots').split()
        if sorted(names) != [name + '.png' for name in SCREENS]:
            raise RuntimeError(f'Unexpected screenshots: {names}')
        images = {name: flatten(dev.adb(serial, 'exec-out', 'run-as', dev.PACKAGE, 'cat', f'cache/store-screenshots/{name}.png')) for name in SCREENS}
        apk = dev.installed_apk_sha256(serial)
    finally:
        demo('exit')
        shell('wm', 'size', 'reset')
        for table in ('global', 'system'):
            for key, value in original[table].items():
                shell('settings', 'delete' if value == 'null' else 'put', table, key, *([] if value == 'null' else [value]))
        if original['timezone']:
            shell('cmd', 'alarm', 'set-timezone', original['timezone'])
        shell('cmd', 'uimode', 'night', original['night'] if original['night'] in ('yes', 'no', 'auto') else 'no')

    output.mkdir(parents=True, exist_ok=True)
    for stale in output.glob('*.png'):
        if stale.stem not in SCREENS:
            stale.unlink()
    for name, image in images.items():
        image.save(output / f'{name}.png', optimize=True)
    receipt = {'serial': serial, 'package': dev.PACKAGE, 'apk_sha256': apk, 'output': str(output),
               'screenshots': {name: hashlib.sha256((output / f'{name}.png').read_bytes()).hexdigest() for name in SCREENS}}
    (private / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


def compare(first: Path, second: Path, tolerance: float):
    """Same files, sizes and modes; differing pixels at most `tolerance` of each image."""
    names = sorted(path.name for path in first.glob('*.png'))
    if names != sorted(path.name for path in second.glob('*.png')) or not names:
        raise SystemExit(f'Different screenshot sets: {names}')
    worst = 0.0
    for name in names:
        a, b = Image.open(first / name), Image.open(second / name)
        if a.size != b.size or a.mode != b.mode:
            raise SystemExit(f'{name}: {a.size} {a.mode} vs {b.size} {b.mode}')
        difference = ImageChops.difference(a.convert('RGB'), b.convert('RGB')).convert('L').point(lambda value: 255 if value > 8 else 0)
        changed = difference.histogram()[255] / (a.size[0] * a.size[1])
        worst = max(worst, changed)
        print(f'{name}: {changed:.4%} of pixels differ')
    if worst > tolerance:
        raise SystemExit(f'Screenshots differ by more than {tolerance:.2%}')
    print(f'Equivalent: {len(names)} screenshots within {tolerance:.2%}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    run = commands.add_parser('capture')
    run.add_argument('--serial', required=True)
    run.add_argument('--output', type=Path, default=DEFAULT_OUTPUT)
    run.add_argument('--skip-build', action='store_true', help='use the play debug APKs already built with the AndroidJUnitRunner')
    check = commands.add_parser('compare')
    check.add_argument('first', type=Path)
    check.add_argument('second', type=Path)
    check.add_argument('--tolerance', type=float, default=0.001, help='largest fraction of changed pixels per screenshot')
    args = parser.parse_args()
    if args.command == 'capture':
        capture(args.serial, args.output.resolve(), args.skip_build)
    else:
        compare(args.first, args.second, args.tolerance)


if __name__ == '__main__':
    main()
