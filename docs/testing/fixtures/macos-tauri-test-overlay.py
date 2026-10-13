#!/usr/bin/env python3
"""Build the ignored Mac installer harness with a private Tauri mock fix.

This never edits the shared Cargo registry, task lock or release artifacts.
The only crate change initializes a new field in upstream's test-only mock.
All inputs, the private lock delta and native compiler output are retained.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tomllib

CRATE_SHA = '667b20e2726d572dea2de7370da16e188eb06008faf9a92fab7cdc46791190b5'
MOCK_SHA = '96ccda85b638ebbdb6a7ea30d5cfdf435cc69e62d0bbf7f9bdc03454c7e177fb'
BEFORE = b'        enable_gtk_app_id: false,\n      },'
AFTER = b'        enable_gtk_app_id: false,\n        app_directories_override: Default::default(),\n      },'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def files(root):
    return {str(p.relative_to(root)): digest(p) for p in sorted(root.rglob('*')) if p.is_file()}


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repo', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--target', type=Path, required=True)
    args = p.parse_args()
    assert platform.system() == 'Darwin' and platform.machine() == 'arm64'
    repo = args.repo.resolve(strict=True)
    assert run('git', '-C', str(repo), 'status', '--porcelain', capture_output=True).stdout == b''
    source_head = run('git', '-C', str(repo), 'rev-parse', 'HEAD', capture_output=True).stdout.decode().strip()
    original_lock_sha = digest(repo / 'Cargo.lock')
    original_lock = tomllib.loads((repo / 'Cargo.lock').read_text())
    tauri = [x for x in original_lock['package'] if x['name'] == 'tauri']
    assert len(tauri) == 1 and tauri[0]['version'] == '2.11.5' and tauri[0]['checksum'] == CRATE_SHA
    output = args.output.resolve()
    assert str(output).startswith(str(repo / '.local/native-'))
    output.mkdir()  # Refuse reuse; keep every failed attempt.
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    matches = list((cargo_home / 'registry/cache').glob('*/tauri-2.11.5.crate'))
    assert len(matches) == 1 and digest(matches[0]) == CRATE_SHA
    registry_mocks = list((cargo_home / 'registry/src').glob('*/tauri-2.11.5/src/test/mod.rs'))
    assert len(registry_mocks) == 1 and digest(registry_mocks[0]) == MOCK_SHA
    with tarfile.open(matches[0]) as archive:
        names = set()
        for member in archive.getmembers():
            path = Path(member.name)
            assert not path.is_absolute() and '..' not in path.parts and path.parts[0] == 'tauri-2.11.5'
            assert member.name not in names and (member.isfile() or member.isdir())
            names.add(member.name)
        archive.extractall(output, filter='data')
    overlay = output / 'tauri-2.11.5'
    crate_before = files(overlay)
    mock = overlay / 'src/test/mod.rs'
    assert digest(mock) == MOCK_SHA and mock.read_bytes().count(BEFORE) == 1
    mock.write_bytes(mock.read_bytes().replace(BEFORE, AFTER))
    crate_after = files(overlay)
    assert crate_before.keys() == crate_after.keys()
    assert [x for x in crate_before if crate_before[x] != crate_after[x]] == ['src/test/mod.rs']
    source = output / 'source'
    run('git', 'clone', '--quiet', '--no-hardlinks', '--no-checkout', str(repo), str(source))
    run('git', '-C', str(source), 'checkout', '--quiet', '--detach', source_head)
    # Cargo also builds the desktop binary for this integration test. Retain
    # the existing built frontend byte-for-byte, rather than inventing assets.
    frontend = repo / 'apps/web/dist'
    assert (frontend / 'index.html').is_file()
    assert not any(x.is_symlink() for x in frontend.rglob('*'))
    frontend_files = files(frontend)
    shutil.copytree(frontend, source / 'apps/web/dist')
    assert files(source / 'apps/web/dist') == frontend_files
    patch = 'patch.crates-io.tauri.path=' + json.dumps(str(overlay))
    env = dict(os.environ, CARGO_TARGET_DIR=str(args.target.resolve()))
    for name in ['WRITER_PUBLIC_RELEASE', 'WRITER_GOOGLE_SIGN_IN_CONFIG', 'APPLE_SIGNING_IDENTITY',
                 'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH']:
        env.pop(name, None)
    # Preserve every locked version/edge. Cargo update can re-resolve unrelated
    # platform packages against the host's cached index even with -p tauri.
    lock = source / 'Cargo.lock'
    original_entry = ('name = "tauri"\nversion = "2.11.5"\n'
                      'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                      f'checksum = "{CRATE_SHA}"\n')
    assert lock.read_text().count(original_entry) == 1
    lock.write_text(lock.read_text().replace(original_entry, 'name = "tauri"\nversion = "2.11.5"\n'))
    updated_lock = tomllib.loads((source / 'Cargo.lock').read_text())
    expected_lock = json.loads(json.dumps(original_lock))
    expected_tauri = next(x for x in expected_lock['package'] if x['name'] == 'tauri')
    del expected_tauri['source']
    del expected_tauri['checksum']
    assert updated_lock == expected_lock, 'Private overlay changed unrelated dependency selections'
    lock_diff = run('git', '-C', str(source), 'diff', '--', 'Cargo.lock', capture_output=True).stdout
    (output / 'lock.diff').write_bytes(lock_diff)
    command = ['cargo', 'test', '--locked', '--offline', '--config', patch, '-p',
               'metrix-writer-desktop', '--release', '--test', 'mac_installer_interruption',
               '--no-run', '--message-format=json']
    with (output / 'compiler.jsonl').open('w') as stdout, (output / 'compiler.log').open('w') as stderr:
        result = subprocess.run(command, cwd=source, env=env, stdout=stdout, stderr=stderr)
    (output / 'build.exit').write_text(str(result.returncode) + '\n')
    assert digest(repo / 'Cargo.lock') == original_lock_sha
    assert digest(registry_mocks[0]) == MOCK_SHA and digest(matches[0]) == CRATE_SHA
    assert files(frontend) == frontend_files
    assert run('git', '-C', str(repo), 'status', '--porcelain', capture_output=True).stdout == b''
    receipt = {
        'kind': 'native-test-only-tauri-mock-overlay',
        'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'source_head': source_head, 'crate': 'tauri2.11.5', 'crate_sha256': CRATE_SHA,
        'changed_crate_files': ['src/test/mod.rs'], 'original_mock_sha256': MOCK_SHA,
        'patched_mock_sha256': digest(mock), 'private_lock_diff_sha256': digest(output / 'lock.diff'),
        'production_lock_sha256': digest(repo / 'Cargo.lock'), 'build_exit': result.returncode,
        'retained_frontend_files': len(frontend_files),
        'retained_frontend_inventory_sha256': hashlib.sha256(json.dumps(frontend_files, sort_keys=True).encode()).hexdigest(),
        'production_and_shared_registry_unchanged': True,
        'limits': ['Mock/test initializer only; no production dependency patch or GUI update acceptance.',
                   'Actual private Tauri installer tests still require separate execution evidence.']}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    assert result.returncode == 0, 'Inspect retained native compiler output'
    records = [json.loads(line) for line in (output / 'compiler.jsonl').read_text().splitlines() if line.startswith('{')]
    binaries = [Path(x['executable']) for x in records if x.get('reason') == 'compiler-artifact'
                and x.get('target', {}).get('name') == 'mac_installer_interruption' and x.get('executable')]
    assert len(binaries) == 1
    receipt.update(test_executable=str(binaries[0]), test_executable_sha256=digest(binaries[0]))
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('PASS: private native mock overlay compiled; production lock and shared registry unchanged.')


if __name__ == '__main__':
    main()
