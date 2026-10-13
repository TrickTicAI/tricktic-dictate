#!/usr/bin/env python3
"""Real-model emulator smoke, malformed PCM/model failures and cooperative cancellation."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

import device


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial', required=True)
    p.add_argument('--build', type=Path, required=True)
    p.add_argument('--weights', type=Path, required=True, help='Pinned base.bin')
    p.add_argument('--wav', type=Path, required=True, help='Pinned runtime samples/jfk.wav')
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if device.device_info(a.serial)['ro.kernel.qemu'] != '1':
        raise ValueError('Smoke is restricted to an explicitly selected emulator')
    root = device.private_path(a.output)
    root.mkdir(parents=True, exist_ok=False)
    suite = root / 'suite.json'
    if device.digest(a.wav) != '59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e':
        raise ValueError('Use samples/jfk.wav from the pinned runtime')
    suite.write_text(json.dumps({'clips': [{
        'id': 'public-en', 'wav': str(a.wav.resolve()), 'sha256': device.digest(a.wav), 'mode': 'en',
        'consented': True, 'reference_kind': 'public_reference', 'provenance': 'Public JFK speech in pinned whisper.cpp samples/jfk.wav; tooling fixture only',
        'reference': 'And so my fellow Americans ask not what your country can do for you ask what you can do for your country.'
    }]}))
    args = argparse.Namespace(serial=a.serial, build=a.build, weights=a.weights, model='base', suite=suite,
                              output=root / 'run', emulator_fixture=True, threads=2, repeats=2, minimum_seconds=0)
    device.run_benchmark(args)
    receipt = json.loads((args.output / 'receipt.json').read_text())
    remote = receipt['remote_directory']
    binary = remote + '/bench'
    checks = {}
    # Verify real Android PCM signed decoding, including both extrema.
    pcm = root / 'extremes.pcm'
    pcm.write_bytes(b'\x00\x80\xff\x7f')
    device.adb(a.serial, 'push', str(pcm), remote + '/extremes.pcm')
    result = json.loads(device.shell(a.serial, binary, '--check-pcm', remote + '/extremes.pcm'))
    assert result['samples'] == 2 and result['first'] == -1 and result['last'] == 32767 / 32768
    checks['pcm_extrema'] = True
    for name, data in [('empty', b''), ('odd', b'\x00')]:
        pcm = root / (name + '.pcm')
        pcm.write_bytes(data)
        device.adb(a.serial, 'push', str(pcm), remote + '/' + pcm.name)
        try:
            device.shell(a.serial, binary, '--check-pcm', remote + '/' + pcm.name)
            raise AssertionError('Malformed PCM accepted')
        except subprocess.CalledProcessError:
            checks[name + '_pcm_rejected'] = True
    # A missing model must fail, without modifying the saved input or old result.
    before = {name: device.shell(a.serial, 'sha256sum', remote + '/' + name).split()[0]
              for name in ['public-en.pcm', 'results.jsonl']}
    failed = device.adb(a.serial, 'shell', f'{binary} {remote}/missing.bin {remote}/plan.json 2>/dev/null', check=False)
    assert json.loads(failed.splitlines()[-1])['event'] == 'error'
    checks['missing_model_failed'] = True
    # Keep the model loaded over many repetitions; interrupt after one durable result.
    plan = json.loads((args.output / 'plan.json').read_text())
    plan['repeats'] = 100
    plan_path = root / 'cancel-plan.json'
    plan_path.write_text(json.dumps(plan))
    device.adb(a.serial, 'push', str(plan_path), remote + '/cancel-plan.json')
    command = f'echo $$ > {remote}/pid; exec {binary} {remote}/model.bin {remote}/cancel-plan.json > {remote}/cancel.jsonl 2> {remote}/cancel.log'
    with (root / 'cancel-adb.log').open('w') as log:
        process = subprocess.Popen([os.environ.get('ADB', 'adb'), '-s', a.serial, 'shell', command], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                raw = device.shell(a.serial, 'cat', remote + '/cancel.jsonl', check=False)
                if '"event":"result"' in raw:
                    break
                if process.poll() is not None:
                    raise AssertionError('Inference exited before cancellation')
                time.sleep(1)
            else:
                raise TimeoutError('Smoke timed out waiting for a first result')
            device.stop_owned(a.serial, remote)
            assert process.wait(timeout=30) == 130
        finally:
            if process.poll() is None:
                device.stop_owned(a.serial, remote)
                process.wait(timeout=30)
    device.adb(a.serial, 'pull', remote + '/cancel.jsonl', str(root / 'cancel.jsonl'))
    events = [json.loads(line) for line in (root / 'cancel.jsonl').read_text().splitlines()]
    assert any(e['event'] == 'result' for e in events)
    assert events[-1]['event'] == 'cancelled'
    checks['cancel_preserved_completed_attempt'] = True
    for name, sha in before.items():
        assert device.shell(a.serial, 'sha256sum', remote + '/' + name).split()[0] == sha
    checks['audio_and_prior_results_unchanged'] = True
    (root / 'checks.json').write_text(json.dumps(checks, indent=2))
    print(json.dumps(checks))


if __name__ == '__main__':
    os.umask(0o077)
    main()
