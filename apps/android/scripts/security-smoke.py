#!/usr/bin/env python3
"""Separate-signer Android boundaries on an explicitly owned Writer emulator; no live credentials."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import dev


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--directory', type=Path, required=True, help='New private evidence directory')
    parser.add_argument('--release', action='store_true', help='Test non-debuggable APK with a disposable test signer')
    args = parser.parse_args()
    args.directory = args.directory.expanduser().resolve()
    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    if not args.serial.startswith('emulator-'):
        parser.error('Security fixtures must never run on a phone or personal app data')
    args.directory.mkdir(parents=True, exist_ok=False)
    # Stable per-owned-emulator keys allow repeat installs. Never use the production signer.
    signing = dev.PRIVATE / 'security-signing'
    signing.mkdir(exist_ok=True)
    java = Path(os.environ['JAVA_HOME']) / 'bin'
    signer = dev.SDK / 'build-tools/36.0.0/apksigner'
    def run(command, label, **kwargs):
        result = subprocess.run([str(x) for x in command], cwd=dev.ROOT, capture_output=True, timeout=900, **kwargs)
        (args.directory / (label + '.log')).write_bytes(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(f'{label} failed; see {args.directory}')
        return result.stdout.decode()
    for name in ('attacker', 'release'):
        key = signing / (name + '.p12')
        if not key.exists():
            run([java / 'keytool', '-genkeypair', '-keystore', key, '-storepass', 'fixture-only',
                 '-keypass', 'fixture-only', '-alias', 'fixture', '-keyalg', 'RSA', '-validity', '30',
                 '-dname', 'CN=Writer isolated security fixture'], name + '-keygen')
    properties = signing / 'release.properties'
    properties.write_text(f'storeFile={signing / "release.p12"}\nstorePassword=fixture-only\nkeyAlias=fixture\nkeyPassword=fixture-only\n')
    variant = 'Release' if args.release else 'Debug'
    package = 'com.metrix.writer' if args.release else dev.PACKAGE
    env = dict(os.environ)
    env.pop("WRITER_ANDROID_SIGNING_PROPERTIES", None)
    # Build release with a supplied public test CA too: release must exclude the debug trust override.
    if args.release:
        env['WRITER_ANDROID_SIGNING_PROPERTIES'] = str(properties)
        ca = signing / 'test-ca.pem'
        ca.write_text(run([java / 'keytool', '-exportcert', '-rfc', '-keystore', signing / 'attacker.p12',
                          '-storepass', 'fixture-only', '-alias', 'fixture'], 'public-test-certificate'))
        env['WRITER_ANDROID_TEST_CA'] = str(ca)
    # The self-updating dev flavor keeps package com.metrix.writer; security checks target it.
    command = [dev.ROOT / 'gradlew', '--offline', f':app:assembleDev{variant}',
               f':app:assembleDev{variant}AndroidTest', ':editor:assembleDebug',
               '-PwriterTestRunner=androidx.test.runner.AndroidJUnitRunner']
    if args.release:
        command.append('-PwriterReleaseTest=true')
    run(command, 'build', env=env)
    attacker = args.directory / 'hostile-editor.apk'
    run([signer, 'sign', '--ks', signing / 'attacker.p12', '--ks-pass', 'pass:fixture-only',
         '--out', attacker, dev.ROOT / 'editor/build/outputs/apk/debug/editor-debug.apk'], 'sign-attacker')
    apks = {'writer': dev.ROOT / f'app/build/outputs/apk/dev/{variant.lower()}/app-dev-{variant.lower()}.apk',
            'instrumentation': dev.ROOT / f'app/build/outputs/apk/androidTest/dev/{variant.lower()}/app-dev-{variant.lower()}-androidTest.apk',
            'attacker': attacker}
    identities = {}
    for name, apk in apks.items():
        verify = run([signer, 'verify', '--print-certs', apk], name + '-signer')
        digest = re.search(r'certificate SHA-256 digest: ([a-f0-9]+)', verify).group(1)
        identities[name] = {'sha256': hashlib.sha256(apk.read_bytes()).hexdigest(), 'signer_sha256': digest}
        run([dev.tool('platform-tools/adb'), '-s', args.serial, 'install', '--no-incremental', '-r', apk], 'install-' + name)
    assert identities['writer']['signer_sha256'] != identities['attacker']['signer_sha256']
    classes = ','.join('com.metrix.writer.' + name for name in (
        'AndroidBoundaryTest', 'AndroidStorageSecurityTest', 'ProtectedWindowTest',
        'WriterUiTest#cloudKeyEntryIsProtectedAndNeverRestoredWithActivityState',
        'WriterUiTest#dictionaryCreateEditAndDeleteWithHebrewSnippet'))
    output = run([dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument', '-w',
                  '-e', 'class', classes, package + '.test/androidx.test.runner.AndroidJUnitRunner'], 'security-tests')
    if 'OK (10 tests)' not in output or 'FAILURES' in output:
        raise RuntimeError(f'Security tests failed; see {args.directory}')
    for name, pkg in [('writer', package), ('attacker', 'com.metrix.writer.editor')]:
        run([dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'dumpsys', 'package', pkg], name + '-package')
    receipt = {'passed': True, 'tests': 10, 'serial': args.serial, 'owned_emulator_pid': owner['pid'],
               'sdk': dev.adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').decode().strip(),
               'fingerprint': dev.adb(args.serial, 'shell', 'getprop', 'ro.build.fingerprint').decode().strip(),
               'commit': run(['git', 'rev-parse', 'HEAD'], 'commit').strip(),
               'dirty': bool(run(['git', 'status', '--porcelain'], 'status').strip()),
               'variant': variant, 'artifacts': identities, 'physical_phone': False,
               'release_signer': 'disposable fixture only' if args.release else 'debug'}
    (args.directory / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    os.umask(0o077)
    main()
