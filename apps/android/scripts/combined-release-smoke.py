#!/usr/bin/env python3
"""Exercise the exact upgraded release APK; separate same-signer tests, owned emulator only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import wave

import dev
import release


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--upgrade-receipt', type=Path, required=True)
    parser.add_argument('--model', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not args.serial.startswith('emulator-'):
        raise ValueError('Never install fixture instrumentation on a physical phone')
    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    candidate = release.inspect(args.candidate / 'writer.apk')
    upgrade = json.loads(args.upgrade_receipt.read_text())
    if (not upgrade['passed'] or upgrade['serial'] != args.serial or upgrade['current'] != candidate
            or upgrade['emulator_pid'] != owner['pid']):
        raise ValueError('Require the successful exact-candidate upgrade on this owned emulator')
    dev.PACKAGE = 'com.metrix.writer'
    if dev.installed_apk_sha256(args.serial) != candidate['apk_sha256']:
        raise ValueError('Installed APK differs from the upgrade candidate')
    catalog = json.loads((dev.ROOT / 'benchmark/assets.json').read_text())
    base = next(model for model in catalog['models'] if model['id'] == 'base')
    with args.model.open('rb') as model:
        if hashlib.file_digest(model, 'sha256').hexdigest() != base['sha256']:
            raise ValueError('Supply the pinned base model')
    output = release.external(args.output)
    output.mkdir(parents=True, exist_ok=False)
    # This exact public clip belongs to the required pinned runtime checkout.
    source = Path(os.environ['WRITER_WHISPER_SOURCE']) / 'samples/jfk.wav'
    with wave.open(str(source), 'rb') as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, 16000):
            raise ValueError('Expected public PCM16 mono 16 kHz speech')
        with wave.open(str(output / 'speech.wav'), 'wb') as target:
            target.setparams(wav.getparams())
            target.writeframes(wav.readframes(wav.getnframes()))
    command = [str(dev.ROOT / 'gradlew'), '--offline', '-PwriterReleaseTest=true',
               '-PwriterTestRunner=android.test.InstrumentationTestRunner', ':app:assembleDevReleaseAndroidTest']
    env = dict(os.environ)
    env['WRITER_ANDROID_SIGNING_PROPERTIES'] = str(release.KEYS / 'signing.properties')
    with (output / 'build.log').open('w') as log:
        subprocess.run(command, cwd=dev.ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    test = output / 'framework-test.apk'
    test.write_bytes((dev.ROOT / 'app/build/outputs/apk/androidTest/dev/release/app-dev-release-androidTest.apk').read_bytes())
    signer = release.run([dev.tool('build-tools/36.0.0/apksigner'), 'verify', '--print-certs', test])
    if 'certificate SHA-256 digest: ' + candidate['signer_sha256'] not in signer:
        raise ValueError('Instrumentation signer mismatch')
    def shell(*parts):
        return dev.adb(args.serial, 'shell', *parts).decode().strip()
    def run_tests(name, classes):
        with (output / (name + '.txt')).open('w') as log:
            subprocess.run([dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument',
                            '-w', '-e', 'class', classes, dev.PACKAGE + '.test/android.test.InstrumentationTestRunner'],
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
        result = (output / (name + '.txt')).read_text()
        if 'OK (' not in result or 'FAILURES' in result:
            raise RuntimeError('Failed ' + name + '; inspect its private log')
        return int(re.search(r'OK \((\d+) test', result)[1])
    staging = '/data/local/tmp/writer-release-proof'
    network = {key: shell('settings', 'get', 'global', key) for key in ['airplane_mode_on', 'wifi_on', 'mobile_data']}
    dev.adb(args.serial, 'install', '-r', str(test))
    try:
        shell('mkdir', staging)  # Refuse an existing staging directory.
        dev.adb(args.serial, 'push', str(args.model), staging + '/base.bin')
        dev.adb(args.serial, 'push', str(output / 'speech.wav'), staging + '/speech.wav')
        shell('cmd', 'connectivity', 'airplane-mode', 'enable')
        shell('svc', 'wifi', 'disable')
        shell('svc', 'data', 'disable')
        combined = 'com.metrix.writer.ReleaseCombinedTest'
        inference_count = run_tests('offline-inference', combined + '#testSignedOfflineInferenceAndRetainedModel')
        dev.adb(args.serial, 'install', '-r', str(args.candidate / 'writer.apk'))
        retained_count = run_tests('model-audio-result-retention', combined + '#testRetainedModelAudioAndResult')
        classes = ['PhoneOfflineTest', 'CompanionTest', 'SessionStoreTest', 'MobileConnectionTest',
                   'AudioCaptureTest', 'WriterPresentationTest', 'DirectCloudTest', 'CloudProviderTest',
                   'CustomTextModelsTest', 'WriterStateTest']
        selectors = ['com.metrix.writer.' + name for name in classes]
        framework_count = run_tests('combined-framework', ','.join(selectors))
        if dev.installed_apk_sha256(args.serial) != candidate['apk_sha256']:
            raise ValueError('Tested APK changed')
    finally:
        shell('am', 'force-stop', dev.PACKAGE)
        shell('cmd', 'connectivity', 'airplane-mode', 'enable' if network['airplane_mode_on'] == '1' else 'disable')
        shell('svc', 'wifi', 'enable' if network['wifi_on'] == '1' else 'disable')
        shell('svc', 'data', 'enable' if network['mobile_data'] == '1' else 'disable')
        shell('rm', '-f', staging + '/base.bin', staging + '/speech.wav')
        shell('rmdir', staging)
        dev.adb(args.serial, 'uninstall', dev.PACKAGE + '.test')
    receipt = {'passed': True, 'candidate': candidate, 'serial': args.serial, 'emulator_pid': owner['pid'],
               'framework_tests': framework_count, 'offline_tests': inference_count, 'retention_tests': retained_count,
               'model': base, 'runtime': catalog['runtime'], 'physical_microphone': False,
               'speech_provenance': 'Public JFK sample from the clean pinned whisper.cpp checkout',
               'speech_sha256': hashlib.sha256((output / 'speech.wav').read_bytes()).hexdigest(),
               'cloud_outputs': 'fixtures; actual cloud acceptance remains separate',
               'model_retention': f"same exact APK reinstall after historical code-{upgrade['previous']['version_code']} to code-{candidate['version_code']} upgrade",
               'test_apk_sha256': hashlib.sha256(test.read_bytes()).hexdigest(),
               'test_source_commit': release.run(['git', '-C', dev.ROOT, 'rev-parse', 'HEAD']).strip(),
               'test_source_dirty': bool(release.run(['git', '-C', dev.ROOT, 'status', '--porcelain']).strip())}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
