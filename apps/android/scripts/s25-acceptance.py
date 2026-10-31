#!/usr/bin/env python3
"""Physical floating-button and in-app acceptance on the S25 (MW-ANDROID-06).

Drives the installed app's real floating control (or Home's Start dictation) while a
recording plays from the host next to the phone (an acoustic path: speaker to the phone
microphone). Timings come from the app's content-free dumpsys exports through
s25-baseline.py. The scratch field or Home result is read before and after through the
accessibility tree; its text stays in the private output directory and only word counts,
WER and word ratios are printed.

Use only an isolated scratch editor focused by the maintainer (a scratch Notion page, a WhatsApp
self-chat) or Writer's own Home. Never a diary or a real conversation. --interrupt applies
one disturbance at a fraction of the clip (home, network, crash) and always undoes it.

  s25-acceptance.py run --serial S --adb "ssh macbook-ts /path/adb" \\
      --clip libri-en-5min --suite SUITE.json --target notion --label en-5min-cold --output DIR
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('device', ROOT / 'benchmark/device.py')
device = importlib.util.module_from_spec(spec); spec.loader.exec_module(device)
spec = importlib.util.spec_from_file_location('baseline', ROOT / 'scripts/s25-baseline.py')
baseline = importlib.util.module_from_spec(spec); spec.loader.exec_module(baseline)
FLAVORS = {'com.metrix.writer': 'dev', 'com.metrix.writer.debug': 'debug'}
TERMINAL_DELIVERY = {'delivered', 'recovery', 'unconfirmed'}
TERMINAL_STAGE = {'raw_ready', 'inserted', 'recovery', 'cancelled', 'failed'}


class Phone:
    def __init__(self, adb, serial, package):
        self.adb, self.serial, self.package = shlex.split(adb), serial, package
        self.service = package + '/com.metrix.writer.WriterInputService'
    def shell(self, *args, timeout=60, check=True):
        return subprocess.run([*self.adb, '-s', self.serial, 'shell', *args], check=check, capture_output=True, timeout=timeout).stdout.decode()
    def foreground(self):
        match = re.search(r'mCurrentFocus=Window\{\S+ \S+ ([\w.]+)', self.shell('dumpsys', 'window'))
        return match and match.group(1)
    def tap(self, x, y, expect=None):
        """Taps only while [expect] (a package) is in front, so a gesture can never land in another app or keyboard."""
        if expect and self.foreground() != expect: raise RuntimeError(f'Refusing input: {self.foreground()} is in front, expected {expect}')
        self.shell('input', 'tap', str(int(x)), str(int(y)))
    def capturing(self):
        match = re.search(r'Writer control check: capture=(\w+)', self.shell('dumpsys', 'activity', 'service', self.service))
        return match and match.group(1) == 'true'
    def view(self): return baseline.last_json(self.shell('dumpsys', 'activity', 'service', self.service, 'processing-json')) or {}
    def overlay(self):
        """Frame of Writer's floating window, from window metadata only."""
        frames = []
        for block in re.split(r'\n(?=  Window #\d+)', self.shell('dumpsys', 'window', 'windows')):
            if f'package={self.package} ' not in block or 'ty=APPLICATION_OVERLAY' not in block: continue
            frame = re.search(r' frame=\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]', block)
            if frame and 'isVisible=true' in block: frames.append(tuple(map(int, frame.groups())))
        if not frames: raise RuntimeError('Writer floating control is not visible')
        return max(frames, key=lambda f: (f[2] - f[0]) * (f[3] - f[1]))
    def nodes(self):
        self.shell('uiautomator', 'dump', '/sdcard/writer-acceptance.xml')
        try: xml = self.shell('cat', '/sdcard/writer-acceptance.xml')
        finally: self.shell('rm', '-f', '/sdcard/writer-acceptance.xml')
        return list(ET.fromstring(xml[xml.index('<'):]).iter('node'))
    def press(self, label):
        """Taps the node whose text or description is [label] (Writer's own Home)."""
        for node in self.nodes():
            if label in (node.get('text'), node.get('content-desc')):
                x1, y1, x2, y2 = map(int, re.findall(r'\d+', node.get('bounds')))
                return self.tap((x1 + x2) / 2, (y1 + y2) / 2, expect=self.package)
        raise RuntimeError(f'Missing control: {label}')
    def field(self):
        """Text of the focused editable node in the active window (the scratch field)."""
        nodes = [n for n in self.nodes() if n.get('focused') == 'true']
        editable = [n for n in nodes if 'EditText' in n.get('class', '') or n.get('editable') == 'true'] or nodes
        # WebView editors such as Notion expose no focused node here; Writer's own readback then
        # remains the insertion proof and the text is not scored.
        if not editable: return None, None
        return editable[-1].get('text', ''), editable[-1].get('package', '')
    def home_text(self):
        """Longest text on Writer's Home: the result card's original or polished text."""
        texts = [n.get('text', '') for n in self.nodes() if n.get('package') == self.package]
        return max(texts, key=len, default='')


def inserted(before, after):
    """The text added between two reads of the same field."""
    start = 0
    while start < min(len(before), len(after)) and before[start] == after[start]: start += 1
    end = 0
    while end < min(len(before), len(after)) - start and before[-1 - end] == after[-1 - end]: end += 1
    return after[start:len(after) - end]


def disturb(phone, kind, log):
    """Applies one interruption and returns how to undo it."""
    log['interrupt_at_unix'] = time.time()
    if kind == 'home':
        phone.shell('input', 'keyevent', 'KEYCODE_HOME'); return lambda: None
    if kind == 'network':
        wifi = phone.shell('settings', 'get', 'global', 'wifi_on').strip(); data = phone.shell('settings', 'get', 'global', 'mobile_data').strip()
        phone.shell('svc', 'wifi', 'disable'); phone.shell('svc', 'data', 'disable')
        def restore():
            if wifi != '0': phone.shell('svc', 'wifi', 'enable')
            if data != '0': phone.shell('svc', 'data', 'enable')
            log['network_restored_at_unix'] = time.time()
        return restore
    if kind == 'crash':
        phone.shell('am', 'crash', phone.package, check=False); return lambda: None
    raise ValueError(kind)


def run(a):
    phone = Phone(a.adb, a.serial, a.package)
    out = device.private_path(a.output) / a.label; out.mkdir(parents=True, exist_ok=False)
    clip = {c['id']: c for c in json.loads(device.private_path(a.suite).read_text())['clips']}[a.clip]
    reference = clip.get('reference') if clip.get('reference') not in (None, 'x') else None
    if a.agreement: reference = json.loads(Path(a.agreement).read_text())[a.clip]
    remote = f'/tmp/writer-acceptance-{a.clip}.wav'
    subprocess.run(['scp', '-q', clip['wav'], f'{a.player_host}:{remote}'], check=True, timeout=120)
    log = {}
    interrupt = a.interrupt.split('@') if a.interrupt else None
    in_app = a.target == 'in-app'
    snap = argparse.Namespace(serial=a.serial, adb=a.adb, flavor=FLAVORS[a.package], out=str(out / 'before.json'))
    def undo():
        restore = log.pop('undo', None)
        if restore: restore()
    try:
        if in_app:
            phone.shell('am', 'start', '-W', '-n', a.package + '/com.metrix.writer.MainActivity'); time.sleep(1.5)
            before_text, package = None, a.package
        else:
            before_text, package = phone.field()
        baseline.snapshot(snap)
        previous = phone.view().get('session_id')
        if in_app: start_tap = time.time(); phone.press('Start dictation')
        else:
            left, top, right, bottom = phone.overlay()
            start_tap = time.time(); phone.tap((left + right) / 2, (top + bottom) / 2)
        deadline = time.time() + 15
        while not phone.capturing():
            if time.time() > deadline: raise RuntimeError('Recording did not start')
            time.sleep(0.2)
        started = time.time()
        player = subprocess.Popen([*shlex.split(a.player), remote])
        if interrupt:
            threading.Timer(clip['audio_ms'] / 1000 * float(interrupt[1]), lambda: log.setdefault('undo', disturb(phone, interrupt[0], log))).start()
        player.wait(timeout=clip['audio_ms'] / 1000 + 120)
        time.sleep(0.8)
        if in_app:
            stop_tap = time.time(); phone.press('Stop and transcribe')
        else:
            left, top, right, bottom = phone.overlay()
            # The recording pill is cancel | level | stop; stop is the end edge (left in RTL).
            stop_x = left + (bottom - top) / 2 if a.rtl else right - (bottom - top) / 2
            stop_tap = time.time(); phone.tap(stop_x, (top + bottom) / 2)
        if a.restore_after is not None: time.sleep(a.restore_after); undo()
        view, state, stage = {}, None, None
        deadline = time.time() + a.timeout
        while time.time() < deadline:
            view = phone.view()
            if view.get('session_id') != previous:
                state, stage = (view.get('delivery') or {}).get('state'), view.get('stage')
                if (stage in TERMINAL_STAGE if in_app else state in TERMINAL_DELIVERY) or stage in {'failed', 'cancelled', 'recovery'}: break
            time.sleep(0.5)
        settled = time.time()
        time.sleep(2.5)  # Readback window of the delivery contract.
        undo(); time.sleep(1)
        snap.out = str(out / 'after.json'); baseline.snapshot(snap)
        if in_app: after_text, after_package = phone.home_text(), a.package
        else: after_text, after_package = phone.field()
    finally:
        undo()
        subprocess.run(['ssh', a.player_host, 'rm', '-f', remote], timeout=60)
    if in_app: text = after_text
    else: text = inserted(before_text, after_text) if before_text is not None and after_text is not None else None
    if text is not None: (out / 'text.txt').write_text(text)
    with open(out / 'diff.json', 'w') as handle:
        sys.stdout, saved = handle, sys.stdout
        try: baseline.diff(argparse.Namespace(before=str(out / 'before.json'), after=str(out / 'after.json'), label=a.label))
        finally: sys.stdout = saved
    words = len(device.tokens(text)) if text is not None else None
    result = {'label': a.label, 'clip': a.clip, 'target': a.target, 'package': a.package, 'interrupt': a.interrupt,
              'field_package': package, 'same_field_package': package == after_package, 'audio_ms': clip['audio_ms'],
              'host_tap_to_capture_ms': round((started - start_tap) * 1000), 'host_stop_to_terminal_ms': round((settled - stop_tap) * 1000),
              'stage': stage, 'delivery_state': state, 'delivery_version': (view.get('delivery') or {}).get('version'),
              'polish': view.get('polish'), 'error': view.get('error'), 'timings_ms': view.get('timings_ms'), 'words': words,
              'wer': round(device.wer(reference, text), 4) if reference and words else None,
              'word_ratio': round(words / len(device.tokens(reference)), 3) if reference and words is not None else None,
              'reference_kind': 'agreement_with_whole_file' if a.agreement else clip.get('reference_kind'),
              'inserted_once': text.count(text[:40]) == 1 if text and len(text) >= 40 else None}
    (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    r = sub.add_parser('run')
    r.add_argument('--serial', required=True); r.add_argument('--adb', required=True)
    r.add_argument('--package', choices=FLAVORS, default='com.metrix.writer')
    r.add_argument('--player', default='ssh macbook-ts afplay'); r.add_argument('--player-host', default='macbook-ts')
    r.add_argument('--suite', required=True); r.add_argument('--clip', required=True)
    r.add_argument('--agreement', help='JSON map of clip id to a whole-file transcript used as the comparison text')
    r.add_argument('--target', required=True, choices=['notion', 'whatsapp', 'in-app'])
    r.add_argument('--interrupt', help='KIND@FRACTION of the clip: home, network or crash')
    r.add_argument('--restore-after', type=float, help='Seconds after Stop to undo the interruption (network)')
    r.add_argument('--label', required=True); r.add_argument('--output', required=True)
    r.add_argument('--rtl', action='store_true'); r.add_argument('--timeout', type=float, default=120)
    a = p.parse_args()
    run(a)


if __name__ == '__main__':
    main()
