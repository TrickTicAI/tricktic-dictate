#!/usr/bin/env python3
"""WSL runner: drive scripts/bench/desktop-replay.ps1 for one staged app build over
a set of PCM16 WAV clips, each in a fresh isolated root, and collect non-content
receipts. The app directory must hold a metrix-writer-desktop.exe built with the
latency-probe feature next to runtime/ and runtime-manifest.json. The template
directory holds State/ (settings, models registry, onboarding, chords that do not
collide with the live app) and optional settings-<lang>.json overrides chosen by
each clip's two-letter language prefix. Model weights are hard-linked per root.
Audio, roots and receipts stay outside source."""
import argparse
import glob
import json
import os
import shutil
import subprocess
import time
from pathlib import Path

PS = '/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'


def idle_seconds():
    """Seconds since the last keyboard or mouse input on the Windows session."""
    script = ('Add-Type @"\nusing System;using System.Runtime.InteropServices;public static class I{[StructLayout(LayoutKind.Sequential)]public struct L{public uint cb;public uint t;}[DllImport("user32.dll")]public static extern bool GetLastInputInfo(ref L l);}\n"@; '
              '$l=New-Object I+L; $l.cb=8; [void][I]::GetLastInputInfo([ref]$l); [int](([Environment]::TickCount - $l.t)/1000)')
    try:
        return int(subprocess.run([PS, '-NoProfile', '-NonInteractive', '-Command', script], cwd='/mnt/c/Temp', capture_output=True, text=True, timeout=30).stdout.strip() or 0)
    except Exception:  # noqa: BLE001 - treat an unreadable idle time as busy
        return 0


def win(path):
    return subprocess.check_output(['wslpath', '-w', str(path)], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-dir', type=Path, required=True)
    parser.add_argument('--template', type=Path, required=True)
    parser.add_argument('--models-dir', type=Path, required=True, help='directory holding the weights files to hard-link')
    parser.add_argument('--model', action='append', default=[], help='lang=weights file name, e.g. en=ggml-large-v3-turbo-q5_0.bin')
    parser.add_argument('--samples', required=True, help='glob of PCM16 WAV clips named <lang>-...')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--replay-script', type=Path, default=Path(__file__).with_name('desktop-replay.ps1'))
    parser.add_argument('--timeout', type=int, default=240)
    parser.add_argument('--require-idle-seconds', type=int, default=60, help='start a trial only after this much keyboard/mouse idle time; 0 disables')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    if any(p.resolve().is_relative_to(repo) for p in (args.output, args.template, args.app_dir)):
        parser.error('Keep the app copy, template, roots and receipts outside source')
    models = dict(m.split('=', 1) for m in args.model)
    script = Path('/mnt/c/Temp/desktop-replay.ps1')
    script.parent.mkdir(exist_ok=True)
    shutil.copy(args.replay_script, script)
    results = []
    for clip in sorted(glob.glob(args.samples)):
        name = Path(clip).stem
        lang = name[:2]
        base = args.output / args.label / f'{name}-{int(time.time())}'
        root, out = base / 'root', base / 'out'
        (root / 'State').mkdir(parents=True)
        (root / 'Models').mkdir()
        for f in (args.template / 'State').iterdir():
            shutil.copy(f, root / 'State' / f.name)
        override = args.template / f'settings-{lang}.json'
        if override.exists():
            shutil.copy(override, root / 'State' / 'settings.json')
        weights = models.get(lang)
        if weights:
            subprocess.run([PS, '-NoProfile', '-NonInteractive', '-Command', f'New-Item -ItemType HardLink -Path "{win(root)}\\Models\\{weights}" -Target "{win(args.models_dir)}\\{weights}" | Out-Null'], cwd='/mnt/c/Temp', stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        command = [PS, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', win(script), '-Executable', win(args.app_dir / 'metrix-writer-desktop.exe'), '-Root', win(root), '-Sample', win(clip), '-OutputDirectory', win(out)]
        # A trial that runs while someone types fails on the modifier-release or
        # foreground guard and says so in its receipt; wait for an idle session.
        while args.require_idle_seconds and idle_seconds() < args.require_idle_seconds:
            time.sleep(10)
        started = time.time()
        with open(base / 'runner.log', 'w') as log:
            process = subprocess.Popen(command, cwd='/mnt/c/Temp', stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
            try:
                code = process.wait(timeout=args.timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                code = 'timeout'
        row = dict(clip=name, build=args.label, exit_code=code, seconds=round(time.time() - started, 1))
        try:
            receipt = json.loads((out / 'result.json').read_text(encoding='utf-8-sig'))
            delivery = receipt.get('delivery') or {}
            stages = delivery.get('stages') or {}
            events = stages.get('events') or []
            stop = stages.get('stop_elapsed_ms') or 0
            after = [(e['stage'], e['elapsed_ms'] - stop) for e in events if e['elapsed_ms'] >= stop]
            first = lambda stage: next((ms for name, ms in after if name == stage), None)
            row.update(field_match=receipt.get('field_newline_normalized'), pasted=delivery.get('pasted'),
                       stop_to_delivery_ms=delivery.get('stop_to_delivery_ms'), stop_to_final_text_ms=first('mode_result_durable'),
                       stop_to_paste_ms=first('paste_sent') if first('paste_sent') is not None else first('delivery_finished'),
                       stages_after_stop=after, details=stages.get('details'),
                       webview_ms=(receipt.get('webview') or {}).get('stop_command_to_receipt_ms'))
        except Exception as error:  # noqa: BLE001 - a failed trial is a recorded result
            row['error'] = str(error)[:200]
        results.append(row)
        print(json.dumps({k: v for k, v in row.items() if k not in ('stages_after_stop', 'details')}), flush=True)
        (args.output / f'{args.label}-results.json').write_text(json.dumps(results, indent=1))
    print('trials', len(results), 'failures', sum(1 for r in results if r.get('field_match') is not True))


if __name__ == '__main__':
    main()
