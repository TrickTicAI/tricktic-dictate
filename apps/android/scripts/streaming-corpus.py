#!/usr/bin/env python3
"""Physical candidate-versus-baseline corpus runs (MW-ANDROID-05, R-1).

Plays each consented/public WAV through the debug candidate's production capture -> streaming ->
job -> polishing path at real-time pace (StreamingCorpusRunner), once with streaming during capture
(candidate) and once with today's whole-file request after Stop (baseline), on the same phone and
network. Uses the debug app's own Gemini key and polishing settings without changing them, never
inserts text and deletes each run's session afterwards.

  streaming-corpus.py --serial S --suite suite.json --clips libri-en-5min ... \
      --output ~/.local/share/metrix-writer/mw-android-05/physical/run-1 --physical-authorized \
      [--adb "ssh host /path/adb"] [--modes streaming whole] [--iterations 3] [--label repeated]

Build the test APK with this runner first:
  ./gradlew :app:assembleDevDebug :app:assembleDevDebugAndroidTest -PwriterTestRunner=com.metrix.writer.StreamingCorpusRunner
The debug app must already use Gemini with its key (Settings › API keys); this script never adds one.

The suite JSON is MW-ANDROID-01's prepare-public-suite.py output ({clips: [{id, wav, mode, reference}]}).
Results with transcripts stay in the private output directory; summary.json holds only timings,
WER/word ratios, receipts, battery and thermal values.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import shlex
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = 'com.metrix.writer.debug'
spec = importlib.util.spec_from_file_location('device', ROOT / 'benchmark/device.py')
device = importlib.util.module_from_spec(spec); spec.loader.exec_module(device)


def run(adb, serial, *args, timeout=900, check=True, data=None):
    command = [*adb, '-s', serial, *args]
    # ssh joins its arguments into one remote shell line: quote them so redirects reach the device.
    if adb[0] == 'ssh': command = adb[:2] + [shlex.join(command[2:])]
    return subprocess.run(command, check=check, capture_output=True, timeout=timeout, input=data)


def state(adb, serial):
    """Battery level/temperature/charging and thermal status only."""
    battery = run(adb, serial, 'shell', 'dumpsys', 'battery').stdout.decode()
    def field(name):
        for line in battery.splitlines():
            if line.strip().startswith(name + ':'): return line.split(':', 1)[1].strip()
    thermal = run(adb, serial, 'shell', 'dumpsys', 'thermalservice').stdout.decode()
    status = next((line.split(':', 1)[1].strip() for line in thermal.splitlines() if line.startswith('Thermal Status:')), None)
    return {'at_unix': int(time.time()), 'level': field('level'), 'temperature_decic': field('temperature'),
            'usb_powered': field('USB powered'), 'ac_powered': field('AC powered'), 'thermal_status': status}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--serial', required=True); p.add_argument('--adb', default='adb')
    p.add_argument('--suite', type=Path, required=True); p.add_argument('--clips', nargs='+', required=True)
    p.add_argument('--modes', nargs='+', choices=['streaming', 'whole'], default=['streaming', 'whole'])
    p.add_argument('--iterations', type=int, default=1); p.add_argument('--label', default='repeated')
    p.add_argument('--drop-at', type=float, help='Streaming only: really close the live connection at this fraction of the audio (fallback budget)')
    p.add_argument('--polish', help='provider/model for this run only (default: the debug app\'s own polishing setting)')
    p.add_argument('--polish-reasoning', default='provider_default')
    p.add_argument('--output', type=Path, required=True); p.add_argument('--install', action='store_true', help='Install this build\'s debug APKs first (never uninstalls)')
    p.add_argument('--physical-authorized', action='store_true', help='Explicit consent to run on the named phone')
    a = p.parse_args()
    adb = shlex.split(a.adb)
    emulator = run(adb, a.serial, 'shell', 'getprop', 'ro.kernel.qemu').stdout.decode().strip() == '1'
    if not emulator and not a.physical_authorized: p.error('Physical runs require --physical-authorized')
    out = device.private_path(a.output); out.mkdir(parents=True, exist_ok=False)
    clips = {c['id']: c for c in json.loads(device.private_path(a.suite).read_text())['clips']}
    if a.install:
        if len(adb) != 1: p.error('--install needs a local adb; copy the APKs to the remote host and install them there')
        for apk in ('app/build/outputs/apk/dev/debug/app-dev-debug.apk', 'app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk'):
            result = run(adb, a.serial, 'install', '-r', '-t', str(ROOT / apk), check=False)
            if result.returncode: sys.exit(f'Install of {apk} failed; nothing was uninstalled: {result.stderr.decode()[-300:]}')
    rows = []; before = state(adb, a.serial)
    for iteration in range(a.iterations):
        for clip_id in a.clips:
            clip = clips[clip_id]
            name = f'corpus-{clip_id}.wav'
            # Copy into the debug app's private files dir through run-as; nothing else on the phone is touched.
            with open(clip['wav'], 'rb') as audio: run(adb, a.serial, 'exec-in', 'run-as', PACKAGE, 'sh', '-c', f'cat > files/{name}', data=audio.read())
            for mode in a.modes:
                started = time.time()
                done = run(adb, a.serial, 'shell', 'am', 'instrument', '-w', '-e', 'wav', name, '-e', 'language', clip['mode'] if clip['mode'] in ('en', 'he') else 'mixed',
                           '-e', 'mode', mode, '-e', 'label', a.label, *(['-e', 'drop_at', str(a.drop_at)] if a.drop_at is not None and mode == 'streaming' else []),
                           *(['-e', 'polish', a.polish, '-e', 'polish_reasoning', a.polish_reasoning] if a.polish else []),
                           f'{PACKAGE}.test/com.metrix.writer.StreamingCorpusRunner', check=False).stdout.decode()
                row = {'clip': clip_id, 'mode': mode, 'iteration': iteration, 'label': a.label, 'wall_s': round(time.time() - started, 1)}
                if 'result=PASS' not in done.replace(' ', ''):
                    row['error'] = next((line.split('result=', 1)[1] for line in done.splitlines() if 'result=' in line), done.strip()[-300:] or 'no result')[:300]
                else:
                    result = json.loads(run(adb, a.serial, 'shell', f'run-as {PACKAGE} cat files/corpus-result.json').stdout)
                    (out / f'{clip_id}-{mode}-{iteration}.json').write_text(json.dumps(result, indent=2))
                    timing = result['timing']; ref = clip.get('reference')  # The maintainer's own clips have no reference text.
                    row.update(audio_ms=result['audio_ms'], transport=result['transport'], recognition=result['recognition'],
                               stop_to_raw_ms=timing.get('stop_to_raw_ms'), stop_to_result_ms=timing.get('stop_to_result_ms'),
                               wer=round(device.wer(ref, result['raw_text']), 4) if ref else None,
                               word_ratio=round(len(device.tokens(result['raw_text'])) / len(device.tokens(ref)), 3) if ref else None,
                               words=len(device.tokens(result['raw_text'])), drop_at=a.drop_at if mode == 'streaming' else None,
                               polish=result['polish'], polish_initial=(result['polish_initial'] or {}).get('reason'), delivery_version=result['delivery_version'])
                print(json.dumps({k: v for k, v in row.items() if k not in ('recognition', 'polish')}), flush=True)
                rows.append(row)
            run(adb, a.serial, 'shell', f'run-as {PACKAGE} rm -f files/{name} files/corpus-result.json', check=False)
    after = state(adb, a.serial)
    summary = {'device': {'emulator': emulator, 'model': run(adb, a.serial, 'shell', 'getprop', 'ro.product.model').stdout.decode().strip()},
               'battery_thermal': {'before': before, 'after': after}, 'rows': rows, 'by_clip_mode': {}}
    for key in sorted({(r['clip'], r['mode']) for r in rows}):
        values = [r for r in rows if (r['clip'], r['mode']) == key and 'stop_to_raw_ms' in r and r['stop_to_raw_ms'] is not None]
        if values:
            latency = sorted(r['stop_to_raw_ms'] for r in values)
            summary['by_clip_mode']['/'.join(key)] = {'n': len(values), 'stop_to_raw_ms': latency,
                'p50_ms': statistics.median(latency), 'wer': [r['wer'] for r in values], 'word_ratio': [r['word_ratio'] for r in values]}
    (out / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary['by_clip_mode'], indent=2))


if __name__ == '__main__':
    main()
