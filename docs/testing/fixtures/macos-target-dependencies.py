#!/usr/bin/env python3
"""Retain an actual Mac desktop dependency graph; not binary-link acceptance.

Run on laptop-main from clean committed source before the release build. This
does not itself justify a disposition: pair it with actual compiler/link and
final-artifact evidence. No credential, permission or installed app is accessed.
"""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import subprocess

PACKAGES = ['glib@0.18.5', 'proc-macro-error@1.0.4', 'unic-char-range@0.9.0',
            'unic-common@0.9.0', 'unic-char-property@0.9.0',
            'unic-ucd-version@0.9.0', 'unic-ucd-ident@0.9.0']


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    root = Path(__file__).resolve().parents[3]
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        p.error('Run on the actual Apple Silicon native build host')
    output = a.output.resolve()
    if output.exists() or not output.is_relative_to(root / '.local'):
        p.error('Use a new ignored receipt path in this task worktree .local directory')
    if subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain']):
        p.error('Use clean committed task source')
    source = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    lock = root / 'Cargo.lock'
    lock_bytes = lock.read_bytes()
    common = ['cargo', 'tree', '--locked', '-p', 'metrix-writer-desktop',
              '--target', 'aarch64-apple-darwin', '--edges', 'normal,build',
              '--features', 'custom-protocol,google-sign-in', '--prefix', 'none']
    result = {'schema_version': 1, 'kind': 'native-target-dependency-graph-not-binary-link-proof',
              'captured_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'host': {'system': platform.system(), 'architecture': platform.machine()},
              'source_commit': source, 'cargo_lock_sha256': hashlib.sha256(lock_bytes).hexdigest(),
              'rustc': subprocess.check_output(['rustc', '-vV'], text=True),
              'cargo': subprocess.check_output(['cargo', '--version'], text=True),
              'target': 'aarch64-apple-darwin', 'packages': []}
    whole = subprocess.run(common, cwd=root, capture_output=True, text=True, timeout=180)
    if whole.returncode:
        p.error('Native target graph failed; no absence proof issued')
    result['native_graph_sha256'] = hashlib.sha256(whole.stdout.encode()).hexdigest()
    locked = {pkg['name'] + '@' + pkg['version'] for pkg in
              __import__('tomllib').loads(lock_bytes.decode())['package']}
    for package in PACKAGES:
        command = common + ['-i', package]
        proc = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=180)
        if package not in locked:
            # Absence comes from the parsed locked package set, not arbitrary
            # nonzero Cargo output or a guessed stderr message.
            evidence = 'removed-from-lock'
        elif proc.returncode:
            p.error('Locked package query failed; no absence proof issued')
        elif proc.stdout.strip():
            evidence = 'present-in-native-target-graph'
        elif proc.stderr.strip() == 'warning: nothing to print.\n\nTo find dependencies that require specific target platforms, try to use option `--target all` first, and then narrow your search scope accordingly.':
            evidence = 'absent-from-native-target-graph'
        else:
            p.error('Unrecognized native dependency query result')
        result['packages'].append({'package': package, 'command': command, 'exit_code': proc.returncode,
                                   'graph': proc.stdout if package in locked else '', 'evidence': evidence})
    if lock.read_bytes() != lock_bytes or subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain']):
        p.error('Source or lock changed during graph capture')
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as f:
        f.write(json.dumps(result, indent=2) + '\n')
    print('Retained native graph for', len(PACKAGES), 'packages; actual compiler/link proof remains required.')


if __name__ == '__main__':
    main()
