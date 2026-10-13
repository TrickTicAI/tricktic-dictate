#!/usr/bin/env python3
"""Explicit-device Writer development. Never restarts adb or selects an arbitrary device."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
SDK = Path(os.environ.get('ANDROID_HOME', os.environ.get('ANDROID_SDK_ROOT', str(Path.home() / 'Android/Sdk'))))
PRIVATE = Path(os.environ.get('WRITER_ANDROID_DEV', str(Path.home() / '.local/share/metrix-writer/android-dev')))
API = os.environ.get('WRITER_ANDROID_API', '36')
if API not in ('33', '34', '36'): raise ValueError('Writer emulator loop supports API 33, 34 or 36')
AVD = 'Writer_API' + API
IMAGE = f'system-images;android-{API};google_apis;x86_64'
# dev is the maintainer's self-updating build; play is the Google Play app. Both debug builds install side by side.
FLAVOR = os.environ.get('WRITER_ANDROID_FLAVOR', 'dev')
if FLAVOR not in ('dev', 'play'): raise ValueError('WRITER_ANDROID_FLAVOR must be dev or play')
PACKAGE = {'dev': 'com.metrix.writer', 'play': 'com.tricktic.dictate'}[FLAVOR] + '.debug'
APP_APK = f'app/build/outputs/apk/{FLAVOR}/debug/app-{FLAVOR}-debug.apk'
TEST_APK = f'app/build/outputs/apk/androidTest/{FLAVOR}/debug/app-{FLAVOR}-debug-androidTest.apk'
DEBUG_TASKS = [f':app:assemble{FLAVOR.capitalize()}Debug', f':app:assemble{FLAVOR.capitalize()}DebugAndroidTest']
EXE = '.exe' if os.name == 'nt' else ''

def tool(path): return str(SDK / (path + EXE))
def run(args, **kwargs): return subprocess.run(args, check=True, capture_output=True, timeout=120, **kwargs).stdout
def adb(serial, *args):
    if not serial: raise ValueError('An explicit --serial is required')
    return run([tool('platform-tools/adb'), '-s', serial, *args])
def installed_apk_sha256(serial):
    # Hash the tested device's APK, not a build output another local run can replace.
    paths = adb(serial, 'shell', 'pm', 'path', PACKAGE).decode().splitlines()
    if len(paths) != 1 or not paths[0].startswith('package:/data/app/') or not paths[0].endswith('/base.apk'):
        raise ValueError('Expected one installed Writer base APK')
    result = adb(serial, 'shell', 'sha256sum', paths[0].removeprefix('package:')).decode().split()[0]
    if len(result) != 64 or any(c not in '0123456789abcdef' for c in result):
        raise ValueError('Invalid installed APK checksum')
    return result

def selected(serial):
    if adb(serial, 'get-state').strip() != b'device': raise ValueError('Device is not ready')
def receipt_path(serial): return PRIVATE / (serial + '.json')
def owned(serial):
    receipt = json.loads(receipt_path(serial).read_text())
    if receipt['serial'] != serial or receipt['avd'] != AVD: raise ValueError('Not Writer-owned')
    if adb(serial, 'emu', 'avd', 'name').decode().splitlines()[0] != AVD: raise ValueError('AVD identity changed')
    os.kill(receipt['pid'], 0)
    cmdline = Path(f"/proc/{receipt['pid']}/cmdline")
    if cmdline.exists():
        args = cmdline.read_bytes().split(b'\0')
        if AVD.encode() not in args or str(receipt['port']).encode() not in args:
            raise ValueError('Recorded PID no longer owns this AVD')
    return receipt

def start(port):
    if port < 5554 or port > 5682 or port % 2: raise ValueError('Use an even emulator port 5554..5682')
    PRIVATE.mkdir(parents=True, exist_ok=True)
    serial = f'emulator-{port}'
    devices = run([tool('platform-tools/adb'), 'devices']).decode()
    if serial in devices: raise ValueError('Serial is already running; use its explicit serial or choose an idle port')
    for number in (port, port + 1):
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(('127.0.0.1', number))
    avd_home = PRIVATE / 'avd'
    avd_home.mkdir(exist_ok=True)
    env = {**os.environ, 'ANDROID_AVD_HOME': str(avd_home)}
    # Reserve before either creating or launching the AVD.
    lock = PRIVATE / 'active.json'
    with lock.open('x') as out: json.dump({'serial': serial}, out)
    process = None
    try:
        marker = avd_home / 'writer-owned.json'
        identity = {'avd': AVD, 'image': IMAGE}
        if (avd_home / f'{AVD}.ini').exists():
            if not marker.exists() or json.loads(marker.read_text()) != identity:
                raise ValueError('Refusing an unmarked AVD')
        else:
            run([tool('cmdline-tools/latest/bin/avdmanager').replace('.exe', '.bat'), 'create', 'avd', '-n', AVD,
                 '-k', identity['image'], '--device', 'pixel_6'], input=b'no\n', env=env)
            marker.write_text(json.dumps(identity))
        with (PRIVATE / f'{serial}.log').open('ab') as log:
            process = subprocess.Popen([tool('emulator/emulator'), '-avd', AVD, '-port', str(port),
                '-no-window', '-no-audio', '-no-snapshot', '-gpu', 'swiftshader', '-memory', '2048', '-cores', '2'],
                env=env, stdout=log, stderr=log, stdin=subprocess.DEVNULL, start_new_session=True)
        receipt = {'serial': serial, 'pid': process.pid, 'avd': AVD, 'image': IMAGE, 'port': port}
        receipt_path(serial).write_text(json.dumps(receipt, indent=2))
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            if process.poll() is not None: raise RuntimeError(f'Emulator exited; inspect {PRIVATE / (serial + ".log")}')
            try:
                if adb(serial, 'shell', 'getprop', 'sys.boot_completed').strip() == b'1':
                    print(json.dumps(receipt)); return
            except subprocess.CalledProcessError: pass
            time.sleep(2)
        raise TimeoutError('Writer emulator did not boot in 240 seconds')
    except BaseException:
        if process and process.poll() is None: process.terminate(); process.wait(timeout=30)
        lock.unlink(missing_ok=True)
        raise

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('command', choices=['devices', 'start', 'stop', 'build', 'install', 'test', 'collect'])
    p.add_argument('--serial')
    p.add_argument('--port', type=int, default=5582)
    a = p.parse_args()
    if a.command == 'devices': print(run([tool('platform-tools/adb'), 'devices', '-l']).decode()); return
    if a.command == 'start': start(a.port); return
    if a.command == 'build':
        subprocess.run([str(ROOT / ('gradlew.bat' if os.name == 'nt' else 'gradlew')), *DEBUG_TASKS, ':editor:assembleDebug'], cwd=ROOT, check=True); return
    selected(a.serial)
    if a.command == 'stop':
        receipt = owned(a.serial)
        adb(a.serial, 'emu', 'kill')
        for _ in range(30):
            try:
                for port in (receipt['port'], receipt['port'] + 1):
                    with socket.socket() as sock:
                        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                        sock.bind(('127.0.0.1', port))
                break
            except OSError: time.sleep(1)
        else: raise TimeoutError('Owned emulator ports have not been released; reservation retained')
        (PRIVATE / 'active.json').unlink(missing_ok=True)
    elif a.command == 'install':
        for apk in (APP_APK, TEST_APK, 'editor/build/outputs/apk/debug/editor-debug.apk'):
            print(adb(a.serial, 'install', '-r', str(ROOT / apk)).decode())
        print(adb(a.serial, 'shell', 'am', 'start', '-W', '-n', PACKAGE + '/com.metrix.writer.MainActivity').decode())
    elif a.command == 'test':
        result = adb(a.serial, 'shell', 'am', 'instrument', '-w', '-e', 'class', 'com.metrix.writer.SessionStoreTest,com.metrix.writer.MobileConnectionTest', PACKAGE + '.test/android.test.InstrumentationTestRunner').decode()
        print(result)
        if 'FAILURES' in result or 'OK (' not in result: raise RuntimeError('Instrumentation failed')
    elif a.command == 'collect':
        apk = ROOT / APP_APK
        print(json.dumps({'serial': a.serial, 'api': adb(a.serial, 'shell', 'getprop', 'ro.build.version.sdk').decode().strip(),
            'abi': adb(a.serial, 'shell', 'getprop', 'ro.product.cpu.abi').decode().strip(),
            'apk_sha256': hashlib.sha256(apk.read_bytes()).hexdigest()}, indent=2))

if __name__ == '__main__': main()
