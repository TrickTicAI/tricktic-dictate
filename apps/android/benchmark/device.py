#!/usr/bin/env python3
"""Pinned assets and explicit-device local ASR qualification. All results stay private."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import subprocess
import time
import unicodedata
import urllib.request
import uuid
import wave

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[2]
ASSETS = json.loads((ROOT / 'assets.json').read_text())


def private_path(path):
    path = Path(path).expanduser().resolve()
    if path == REPO or REPO in path.parents:
        raise ValueError('Audio, models, builds and receipts must be outside the source checkout')
    return path


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify(path, asset):
    if Path(path).stat().st_size != asset['bytes'] or digest(path) != asset['sha256']:
        raise ValueError('Asset size/hash mismatch: ' + asset['id'])


def model_by_id(name):
    return next(m for m in ASSETS['models'] if m['id'] == name)


def fetch_model(name, directory):
    model = model_by_id(name)
    directory = private_path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / (name + '.bin')
    if target.exists():
        verify(target, model)
        return target
    url = f'https://huggingface.co/{model["repository"]}/resolve/{model["revision"]}/{model["filename"]}'
    temporary = directory / (name + '.' + uuid.uuid4().hex + '.part')
    try:
        with urllib.request.urlopen(url, timeout=120) as response, temporary.open('xb') as out:
            while chunk := response.read(1024 * 1024):
                out.write(chunk)
            out.flush()
            os.fsync(out.fileno())
        verify(temporary, model)
        # No partial model ever acquires the verified filename; refuse overwrite.
        os.link(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return target


def pcm_from_wav(path, target):
    with wave.open(str(private_path(path)), 'rb') as audio:
        if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype()) != (1, 2, 16000, 'NONE'):
            raise ValueError('Use Writer-compatible 16 kHz mono PCM16 WAV; no implicit resampling')
        expected = audio.getnframes() * 2
        if expected == 0 or expected // 2 > 2147483647:
            raise ValueError('Empty audio or whisper int sample-count limit exceeded')
        count = 0
        with target.open('xb') as out:
            while chunk := audio.readframes(65536):
                out.write(chunk)
                count += len(chunk)
        if count != expected or count % 2:
            raise ValueError('Truncated WAV frames')
        return count / 32.0


def tokens(text):
    # Explicit normalization: Unicode NFKC, casefold, marks and punctuation removed.
    text = unicodedata.normalize('NFKC', text).casefold()
    text = ''.join(c for c in text if not unicodedata.category(c).startswith('M'))
    return ''.join(c if c.isalnum() else ' ' for c in text).split()


def wer(reference, actual):
    reference, actual = tokens(reference), tokens(actual)
    if not reference:
        return None
    previous = list(range(len(actual) + 1))
    for i, word in enumerate(reference, 1):
        current = [i]
        for j, candidate in enumerate(actual, 1):
            current.append(min(current[-1] + 1, previous[j] + 1, previous[j - 1] + (word != candidate)))
        previous = current
    return previous[-1] / len(reference)


def adb(serial, *args, check=True):
    executable = os.environ.get('ADB', 'adb')
    result = subprocess.run([executable, '-s', serial, *args], check=check, capture_output=True, timeout=300 if args[0] in ('push', 'pull') else 30)
    return result.stdout.decode(errors='replace').strip()


def shell(serial, *args, check=True):
    return adb(serial, 'shell', shlex.join(args), check=check)


def device_info(serial):
    names = ['ro.product.manufacturer', 'ro.product.model', 'ro.product.cpu.abi', 'ro.build.fingerprint',
             'ro.build.version.release', 'ro.build.version.sdk', 'ro.build.version.security_patch',
             'ro.soc.manufacturer', 'ro.soc.model', 'ro.kernel.qemu', 'ro.build.version.oneui']
    info = {name: shell(serial, 'getprop', name) for name in names}
    features = [set(line.split(':', 1)[1].split()) for line in shell(serial, 'cat', '/proc/cpuinfo').splitlines() if line.startswith('Features') and ':' in line]
    info['cpu_features'] = sorted(set.intersection(*features)) if features else []
    return info


def network_state(serial):
    return {'airplane': shell(serial, 'settings', 'get', 'global', 'airplane_mode_on'),
            'wifi': shell(serial, 'settings', 'get', 'global', 'wifi_on'),
            'bluetooth': shell(serial, 'settings', 'get', 'global', 'bluetooth_on'),
            'global_addresses': shell(serial, 'ip', '-o', 'addr', 'show', 'scope', 'global')}


def offline(state):
    return state['airplane'] == '1' and state['wifi'] == '0' and state['bluetooth'] == '0' and not state['global_addresses']


def telemetry(serial):
    return {'host_monotonic': time.monotonic(), 'network': network_state(serial),
            'battery': shell(serial, 'dumpsys', 'battery', check=False),
            'thermal': shell(serial, 'dumpsys', 'thermalservice', check=False)}


def stop_owned(serial, remote):
    pid = shell(serial, 'cat', remote + '/pid', check=False)
    if not pid.isdecimal():
        return
    cmdline = shell(serial, 'cat', f'/proc/{pid}/cmdline', check=False)
    if cmdline.split('\0')[0] == remote + '/bench':
        shell(serial, 'kill', '-INT', pid)


def run_benchmark(a):
    if a.threads < 1 or a.repeats < 1 or a.minimum_seconds < 0 or not math.isfinite(a.minimum_seconds):
        raise ValueError('Invalid run limits')
    output = private_path(a.output)
    suite_path = private_path(a.suite)
    suite = json.loads(suite_path.read_text())
    model = model_by_id(a.model)
    weights = private_path(a.weights)
    verify(weights, model)
    build = private_path(a.build)
    build_receipt = json.loads((build / 'build.json').read_text())
    if build_receipt['runtime'] != ASSETS['runtime'] or digest(build / 'writer-android-bench') != build_receipt['binary_sha256']:
        raise ValueError('Binary/runtime does not match build receipt')
    if adb(a.serial, 'get-state') != 'device':
        raise ValueError('Explicit device is not ready')
    info = device_info(a.serial)
    if int(info['ro.build.version.sdk']) < ASSETS['runtime']['android_api']:
        raise ValueError('Device Android API is below the pinned build floor')
    emulator = info['ro.kernel.qemu'] == '1'
    if a.emulator_fixture and not emulator:
        raise ValueError('--emulator-fixture is forbidden on physical devices')
    if not set(build_receipt.get('required_cpu_features', [])).issubset(info['cpu_features']):
        raise ValueError('Device lacks required ARM CPU instructions')
    if build_receipt['abi'] != info['ro.product.cpu.abi']:
        raise ValueError('Build/device ABI mismatch')
    if not a.emulator_fixture and not offline(network_state(a.serial)):
        raise ValueError('Set airplane mode and turn Wi-Fi/Bluetooth off manually; use USB ADB and remove network interfaces')
    if not suite.get('clips'):
        raise ValueError('No reference clips')
    output.mkdir(parents=True, exist_ok=False)
    remote = '/data/local/tmp/writer-mw63-' + uuid.uuid4().hex
    plan = {'threads': a.threads, 'repeats': a.repeats, 'minimum_seconds': a.minimum_seconds, 'clips': []}
    seen = set()
    for clip in suite['clips']:
        identifier = clip['id']
        if not re.fullmatch(r'[a-zA-Z0-9_-]+', identifier) or identifier in seen:
            raise ValueError('Clip IDs must be unique safe names')
        seen.add(identifier)
        if clip.get('consented') is not True or not clip.get('provenance'):
            raise ValueError('Each clip needs explicit consent/provenance')
        if clip['mode'] not in model['evaluation_modes']:
            raise ValueError('Model is not a candidate for this language mode')
        if clip.get('reference_kind', 'provided_reference') not in ['provided_reference', 'public_reference', 'human_reference', 'prompt_confirmed', 'prompt_unconfirmed', 'model_baseline']:
            raise ValueError('Unknown reference provenance')
        if not isinstance(clip.get('reference'), str) or not clip['reference'].strip():
            raise ValueError('Each clip needs a private reference transcript')
        wav = private_path(clip['wav'])
        if digest(wav) != clip['sha256']:
            raise ValueError('Clip hash mismatch')
        pcm = output / (identifier + '.pcm')
        duration = pcm_from_wav(wav, pcm)
        language = {'he': 'he', 'en': 'en', 'mixed': 'auto'}[clip['mode']]
        plan['clips'].append({'id': identifier, 'pcm': remote + '/' + pcm.name, 'language': language,
                              'audio_ms': duration, 'pcm_sha256': digest(pcm)})
    (output / 'plan.json').write_text(json.dumps(plan, indent=2))
    # Keep references/provenance solely in the private host receipt, never stdout.
    (output / 'suite.json').write_text(json.dumps(suite, ensure_ascii=False, indent=2))
    receipt = {'device': info, 'serial': a.serial, 'remote_directory': remote, 'build': build_receipt,
               'model': model, 'emulator_fixture': a.emulator_fixture,
               'started_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
               'status': 'prepared', 'qualification': 'pending-human-and-physical-review'}
    receipt_path = output / 'receipt.json'
    receipt_path.write_text(json.dumps(receipt, indent=2))
    shell(a.serial, 'mkdir', '-m', '700', remote)
    for source, name in [(build / 'writer-android-bench', 'bench'), (weights, 'model.bin'), (output / 'plan.json', 'plan.json')]:
        adb(a.serial, 'push', str(source), remote + '/' + name)
        if shell(a.serial, 'sha256sum', remote + '/' + name).split()[0] != digest(source):
            raise ValueError('Device asset hash mismatch')
    for pcm in output.glob('*.pcm'):
        adb(a.serial, 'push', str(pcm), remote + '/' + pcm.name)
        if shell(a.serial, 'sha256sum', remote + '/' + pcm.name).split()[0] != digest(pcm):
            raise ValueError('Device PCM hash mismatch')
    shell(a.serial, 'chmod', '700', remote + '/bench')
    command = f'echo $$ > {remote}/pid; exec {remote}/bench {remote}/model.bin {remote}/plan.json > {remote}/results.jsonl 2> {remote}/runtime.log'
    process = None
    try:
        with (output / 'telemetry.jsonl').open('x') as log, (output / 'adb.log').open('x') as adb_log:
            sample = telemetry(a.serial)
            log.write(json.dumps(sample) + '\n'); log.flush()
            if not a.emulator_fixture and not offline(sample['network']):
                raise ValueError('Networking changed during asset staging')
            process = subprocess.Popen([os.environ.get('ADB', 'adb'), '-s', a.serial, 'shell', command], stdout=adb_log, stderr=adb_log)
            while process.poll() is None:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
                sample = telemetry(a.serial)
                log.write(json.dumps(sample) + '\n'); log.flush()
                if not a.emulator_fixture and not offline(sample['network']):
                    raise ValueError('Networking enabled during benchmark; qualification refused')
            receipt['exit_code'] = process.returncode
            receipt['status'] = 'complete' if process.returncode == 0 else 'failed'
    except BaseException:
        receipt['status'] = 'interrupted'
        stop_owned(a.serial, remote)
        raise
    finally:
        receipt_path.write_text(json.dumps(receipt, indent=2))
        if process:
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                # No broad pkill or adb restart; retain PID/path for explicit recovery.
                process.terminate()
            for filename in ['results.jsonl', 'runtime.log']:
                adb(a.serial, 'pull', remote + '/' + filename, str(output / filename), check=False)
    if receipt['status'] != 'complete':
        raise RuntimeError('Inference failed; private audio/results retained at ' + str(output))
    summarize(output)
    print('Private benchmark receipt: ' + str(receipt_path))


def summarize(output):
    output = private_path(output)
    clips = {c['id']: c for c in json.loads((output / 'suite.json').read_text())['clips']}
    rows = []
    complete = False
    for line in (output / 'results.jsonl').read_text().splitlines():
        event = json.loads(line)
        complete = event['event'] == 'complete'
        if event['event'] != 'result':
            continue
        clip = clips[event['clip']]
        rows.append({k: v for k, v in event.items() if k != 'raw_text'})
        kind = clip.get('reference_kind', 'provided_reference')
        score = wer(clip['reference'], event['raw_text'])
        rows[-1].update({'mode': clip['mode'], 'reference_kind': kind,
                         'wer': None if kind in ['model_baseline', 'prompt_unconfirmed'] else score,
                         'baseline_disagreement': score if kind == 'model_baseline' else None,
                         'prompt_disagreement': score if kind == 'prompt_unconfirmed' else None,
                         'rtf': event['inference_ms'] / event['audio_ms'],
                         'meaning_preserved': None, 'omissions': None, 'unwanted_translation': None,
                         'names_correct': None, 'human_corrections': None,
                         'online_baseline_wer': wer(clip['reference'], clip['online_raw_text']) if 'online_raw_text' in clip and kind not in ['model_baseline', 'prompt_unconfirmed'] else None})
    if not complete:
        raise ValueError('No terminal completion record; raw partial results remain private')
    (output / 'quality-review.json').write_text(json.dumps(rows, ensure_ascii=False, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest='command', required=True)
    f = sub.add_parser('fetch-model')
    f.add_argument('--model', choices=[m['id'] for m in ASSETS['models']], required=True)
    f.add_argument('--directory', type=Path, required=True)
    r = sub.add_parser('run')
    r.add_argument('--serial', required=True)
    r.add_argument('--build', type=Path, required=True)
    r.add_argument('--model', choices=[m['id'] for m in ASSETS['models']], required=True)
    r.add_argument('--weights', type=Path, required=True)
    r.add_argument('--suite', type=Path, required=True)
    r.add_argument('--output', type=Path, required=True)
    r.add_argument('--threads', type=int, default=4)
    r.add_argument('--repeats', type=int, default=3)
    r.add_argument('--minimum-seconds', type=float, default=0)
    r.add_argument('--emulator-fixture', action='store_true', help='Tooling smoke only; never physical qualification')
    s = sub.add_parser('summarize')
    s.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if a.command == 'fetch-model':
        print(fetch_model(a.model, a.directory))
    elif a.command == 'run':
        run_benchmark(a)
    else:
        summarize(a.output)


if __name__ == '__main__':
    os.umask(0o077)
    main()
