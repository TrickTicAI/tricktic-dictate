#!/usr/bin/env python3
"""Content-free physical dictation baseline (MW-ANDROID-01).

Reads only what the installed app already exposes through shell dumpsys: the bounded
latency export and the processing view. Never reads recordings, transcripts, editor
text or keys, and never changes phone settings. Works on the non-debuggable dev build.

  s25-baseline.py snapshot --serial S --out DIR/before.json [--adb "ssh host /path/adb"]
  (dictate on the phone)
  s25-baseline.py snapshot --serial S --out DIR/after.json
  s25-baseline.py diff DIR/before.json DIR/after.json --label en-5min-cold > DIR/en-5min-cold.json
"""
import argparse
import json
import re
import shlex
import subprocess
import sys
import time

PACKAGES = {'dev': 'com.metrix.writer', 'play': 'com.tricktic.dictate', 'debug': 'com.metrix.writer.debug'}  # debug: owned emulator only


def shell(adb, serial, *args):
    return subprocess.run([*adb, '-s', serial, 'shell', *args], check=True, capture_output=True, timeout=60).stdout.decode()


def last_json(text):
    start = text.find('{')
    if start < 0: return None
    try: return json.loads(text[start:text.rindex('}') + 1])
    except ValueError: return None  # Older builds lack the dump argument and print state instead.


def snapshot(a):
    adb = shlex.split(a.adb)
    package = PACKAGES[a.flavor]
    service = f'{package}/com.metrix.writer.WriterInputService'
    def prop(name): return shell(adb, a.serial, 'getprop', name).strip()
    battery = shell(adb, a.serial, 'dumpsys', 'battery')
    def field(name):
        match = re.search(rf'^\s*{name}: (\S+)', battery, re.M)
        return match.group(1) if match else None
    thermal = shell(adb, a.serial, 'dumpsys', 'thermalservice')
    status = re.search(r'Thermal Status: (\d+)', thermal)
    package_info = shell(adb, a.serial, 'dumpsys', 'package', package)
    version = re.search(r'versionName=(\S+)', package_info); code = re.search(r'versionCode=(\d+)', package_info)
    latency = last_json(shell(adb, a.serial, 'dumpsys', 'activity', 'service', service, 'latency-json'))
    view = last_json(shell(adb, a.serial, 'dumpsys', 'activity', 'service', service, 'processing-json'))
    if latency is None: sys.exit('Writer text insertion service is not running; enable it so dumpsys can read the export')
    def setting(scope, name): return shell(adb, a.serial, 'settings', 'get', scope, name).strip()
    connectivity = shell(adb, a.serial, 'dumpsys', 'connectivity')  # Network type only, never names or addresses.
    default = re.search(r'Active default network: (\d+)', connectivity)
    transport = default and re.search(rf'NetworkAgentInfo\{{network\{{{default.group(1)}\}}.*?ni\{{([A-Z_]+(?:\[[A-Z0-9_ ]*\])?)', connectivity)
    transport = transport and transport.group(1)
    result = {
        'captured_at_unix': int(time.time()),
        'device': {'model': prop('ro.product.model'), 'fingerprint': prop('ro.build.fingerprint'), 'soc': prop('ro.soc.model'),
                   'emulator': prop('ro.kernel.qemu') == '1'},
        'app': {'package': package, 'version': version and version.group(1), 'code': code and int(code.group(1))},
        'boot_count': setting('global', 'boot_count'),
        'network': {'airplane': setting('global', 'airplane_mode_on'), 'wifi': setting('global', 'wifi_on'), 'default_transport': transport},
        'battery': {'level': field('level'), 'temperature_decic': field('temperature'), 'usb_powered': field('USB powered'), 'ac_powered': field('AC powered')},
        'thermal_status': int(status.group(1)) if status else None,
        'latency': latency, 'processing': view,
    }
    with open(a.out, 'x') as out: json.dump(result, out, indent=2)


def diff(a):
    before, after = (json.load(open(path)) for path in (a.before, a.after))
    known = {(r['attempt_id'], r['provider_attempt']): r for r in before['latency']['records']}
    rows = [r for r in after['latency']['records'] if known.get((r['attempt_id'], r['provider_attempt'])) != r]
    def number(snapshot, *path):
        value = snapshot
        for key in path: value = (value or {}).get(key)
        try: return int(value)
        except (TypeError, ValueError): return None
    level = [number(s, 'battery', 'level') for s in (before, after)]
    temperature = [number(s, 'battery', 'temperature_decic') for s in (before, after)]
    json.dump({
        'label': a.label, 'measurement': 'emulator_fixture' if after['device'].get('emulator') else 'physical_device',
        'device': after['device'], 'app': after['app'], 'network': after['network'],
        'same_boot': before['boot_count'] == after['boot_count'],
        'elapsed_s': after['captured_at_unix'] - before['captured_at_unix'],
        'battery_level_delta': None if None in level else level[1] - level[0],
        'battery_temperature_delta_c': None if None in temperature else (temperature[1] - temperature[0]) / 10,
        'charging': after['battery']['usb_powered'] == 'true' or after['battery']['ac_powered'] == 'true',
        'thermal_status': [before['thermal_status'], after['thermal_status']],
        'records': rows, 'final_view': after['processing'],
    }, sys.stdout, indent=2)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    s = sub.add_parser('snapshot'); s.add_argument('--serial', required=True); s.add_argument('--out', required=True)
    s.add_argument('--adb', default='adb', help='ADB command, e.g. "ssh host /path/to/adb"'); s.add_argument('--flavor', choices=PACKAGES, default='dev')
    d = sub.add_parser('diff'); d.add_argument('before'); d.add_argument('after'); d.add_argument('--label', required=True)
    a = p.parse_args()
    snapshot(a) if a.command == 'snapshot' else diff(a)


if __name__ == '__main__':
    main()
