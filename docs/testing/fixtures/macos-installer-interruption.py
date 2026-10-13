#!/usr/bin/env python3
"""Interrupt the real Mac installer in a new private directory, then recover.

Needs a compiled mac_installer_interruption test binary and retained signed
archives. Defaults to historical 0.37.0/0.38.0; a later candidate requires an
explicit expected hash and version. No live app/profile, release feed, clock or
permissions are changed. All test artifacts are retained for inspection.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import time

OLD_SHA = '80f34f2dc1c07298bf57cbebc27fab47aed5652917e21e139900eef2b91a89c3'
NEW_SHA = 'c0e96adbdba13d26c86da9c16486fd67abfdb5d83ee62bd4068e387b74e03580'
TEST = 'retained_signed_archive_installs_in_private_copy'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    result = {}
    for path in sorted(root.rglob('*')):
        assert not path.is_symlink(), 'Unexpected symlink in fixture bundle'
        if path.is_file():
            result[str(path.relative_to(root))] = digest(path)
    return result


def verify(app):
    subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)],
                   check=True, capture_output=True)


def extract(archive, directory):
    directory.mkdir()
    names = subprocess.check_output(['/usr/bin/tar', '-tzf', str(archive)], text=True).splitlines()
    assert names and all(not Path(n).is_absolute() and '..' not in Path(n).parts for n in names)
    subprocess.run(['/usr/bin/tar', '-xzf', str(archive), '-C', str(directory)], check=True)
    apps = list(directory.glob('*.app'))
    assert len(apps) == 1 and apps[0].name == 'TrickTic Dictate.app'
    verify(apps[0])
    return apps[0]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--old-archive', type=Path, required=True)
    p.add_argument('--new-archive', type=Path, required=True)
    p.add_argument('--test-binary', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--expected-new-sha256', default=NEW_SHA)
    p.add_argument('--expected-new-version', default='0.38.0')
    args = p.parse_args()
    assert platform.system() == 'Darwin' and platform.machine() == 'arm64'
    old, new, binary = (p.resolve(strict=True) for p in
                        [args.old_archive, args.new_archive, args.test_binary])
    assert re.fullmatch(r'[a-f0-9]{64}', args.expected_new_sha256)
    assert re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', args.expected_new_version)
    assert digest(old) == OLD_SHA and digest(new) == args.expected_new_sha256, 'Use exact retained candidates'
    root = args.output.resolve()
    root.mkdir()  # No reuse or cleanup of any existing directory.
    (root / '.fixture').write_text('MW-SEC-22 disposable installer\n')
    (root / 'tmp').mkdir()
    profile = root / 'profile'
    profile.mkdir()
    (profile / 'saved-audio.fixture').write_bytes(b'RIFF synthetic saved audio sentinel\0')
    (profile / 'raw-transcript.fixture').write_text('Synthetic independent raw transcript.\n')
    data_before = inventory(profile)
    app = extract(old, root / 'install')
    old_files = inventory(app)
    expected = extract(new, root / 'expected')
    expected_version = subprocess.check_output(
        ['/usr/libexec/PlistBuddy', '-c', 'Print :CFBundleShortVersionString',
         str(expected / 'Contents/Info.plist')], text=True).strip()
    assert expected_version == args.expected_new_version, 'Candidate bundle version differs'
    expected_files = inventory(expected)
    library = root / 'interrupt.dylib'
    subprocess.run(['/usr/bin/clang', '-Wall', '-Wextra', '-Werror', '-dynamiclib',
                    str(Path(__file__).with_suffix('.c')), '-o', str(library)], check=True)
    env = dict(os.environ)
    for key in ['DYLD_INSERT_LIBRARIES', 'DYLD_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH']:
        env.pop(key, None)
    env.update(WRITER_INSTALL_FIXTURE_ROOT=str(root),
               WRITER_INSTALL_FIXTURE_ARCHIVE=str(new), TMPDIR=str(root / 'tmp'))
    stopped = root / 'backup-path.txt'
    injected = dict(env, DYLD_INSERT_LIBRARIES=str(library),
                    WRITER_INSTALL_STOP_PATH=str(app), WRITER_INSTALL_STOP_RECEIPT=str(stopped))
    command = [str(binary), '--ignored', '--exact', TEST, '--nocapture']
    with (root / 'interrupted.log').open('w') as log:
        child = subprocess.Popen(command, env=injected, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                pid, status = os.waitpid(child.pid, os.WUNTRACED | os.WNOHANG)
                if pid:
                    assert os.WIFSTOPPED(status) and os.WSTOPSIG(status) == signal.SIGSTOP, 'Installer did not stop at boundary'
                    break
                time.sleep(0.02)
            else:
                raise AssertionError('Installer interruption boundary timed out')
            assert stopped.is_file() and not app.exists()
            backup = Path(stopped.read_text()).resolve(strict=True)
            assert backup.is_relative_to(root / 'tmp') and backup.name == 'current_app'
            assert inventory(backup) == old_files
            child.kill()
            assert child.wait(timeout=5) == -signal.SIGKILL
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
    assert not app.exists() and inventory(backup) == old_files
    assert inventory(profile) == data_before
    assert not (root / 'installation-returned').exists()
    # Explicit operator recovery, not automatic rollback: restore the retained
    # original bundle, verify all bytes, then retry the real signed installation.
    os.rename(backup, app)
    verify(app)
    assert inventory(app) == old_files
    with (root / 'retry.log').open('w') as log:
        subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
    verify(app)
    assert inventory(app) == expected_files and inventory(profile) == data_before
    receipt = {
        'schema': 1, 'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'platform': platform.platform(), 'old_version': '0.37.0', 'new_version': expected_version,
        'old_archive_sha256': OLD_SHA, 'new_archive_sha256': args.expected_new_sha256,
        'test_executable_sha256': digest(binary), 'interposer_sha256': digest(library),
        'verified_release': json.loads((root / 'verified-before-install.json').read_text()),
        'interruption': {'boundary': 'after successful old-app rename to native installer backup',
                         'signal': 'SIGKILL after SIGSTOP', 'app_path_absent': True,
                         'original_backup_matches_all_files': True, 'original_file_count': len(old_files),
                         'automatic_rollback_observed': False},
        'recovery': {'method': 'operator restores exact backup, then retries signed installer',
                     'restored_original_codesign_strict_deep': True,
                     'new_bundle_matches_all_files': True, 'new_file_count': len(expected_files),
                     'new_codesign_strict_deep': True},
        'saved_data': {'synthetic_only': True, 'before': data_before, 'after': inventory(profile), 'unchanged': True},
        'limits': ['Actual unmodified Tauri Mac installer with test-only dyld stop hook.',
                   'No full in-app update chain, physical power loss, or permission-prompt acceptance.',
                   'Private signed bundle copies only; live app and real saved data untouched.',
                   f'Retained {expected_version} is not asserted to be published; no network request or release action.']}
    (root / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('PASS: interruption observed, explicit backup recovery and signed retry verified; receipt retained.')


if __name__ == '__main__':
    main()
