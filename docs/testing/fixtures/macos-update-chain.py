#!/usr/bin/env python3
"""Retain exact private native GUI update-chain identities and saved data.

prepare creates a new baseline; launch uses its isolated profile/browser home.
observe records the real running/restarted version; it never installs or calls
mock IPC. UI actions use the separate PID-guarded native AX helper. Real audio,
keys, TCC, normal Writer and every existing fixture remain untouched.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import subprocess
import tarfile

ARCHIVES = {
    '0.19.0': ('0.19.0-c784c0bfa1e1-MBC9gM/Metrix Writer.app.tar.gz', 'd2ad3583156a54b0407c524f950cfccb5a860ecda5b720eca39ba089522e9478'),
    '0.35.0': ('0.35.0-b9adfb5decaf-QMsadg/TrickTic Dictate.app.tar.gz', '5d2da1790d03fd1305d72c73938359f046b58cfb14cc17e930fe85858d40175a'),
    '0.37.0': ('0.37.0-1c984b35a254-13iL6s/TrickTic Dictate.app.tar.gz', '80f34f2dc1c07298bf57cbebc27fab47aed5652917e21e139900eef2b91a89c3'),
    '0.39.0': ('0.39.0-bd772655dee5-30L5T6/TrickTic Dictate.app.tar.gz', '496b1d27ccc47447a5e3e66ca0924aafcca173777f3f84279dc22f6513a79983'),
}


def digest(p):
    with p.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def run(args, **kwargs):
    result = subprocess.run(args, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError('Native fixture operation failed: ' + Path(args[0]).name)
    return result.stdout


def inventory(root):
    assert not any(p.is_symlink() for p in root.rglob('*'))
    return {str(p.relative_to(root)): digest(p) for p in sorted(root.rglob('*')) if p.is_file()}


def write(root, name, value):
    with (root / (name + '.json')).open('x') as f:
        json.dump(value, f, indent=2)
        f.write('\n')
    (root / (name + '.json')).chmod(0o600)


def app_identity(app):
    run(['codesign', '--verify', '--deep', '--strict', str(app)])
    version = run(['/usr/libexec/PlistBuddy', '-c', 'Print :CFBundleShortVersionString', str(app / 'Contents/Info.plist')]).decode().strip()
    bundle_id = run(['/usr/libexec/PlistBuddy', '-c', 'Print :CFBundleIdentifier', str(app / 'Contents/Info.plist')]).decode().strip()
    assert bundle_id == 'com.metrix.writer' and version in ARCHIVES
    executable = app / 'Contents/MacOS/metrix-writer-desktop'
    return {'version': version, 'bundle_id': bundle_id, 'executable_sha256': digest(executable),
            'bundle_files': len(inventory(app))}


def prepare(repo, root, baseline):
    root.mkdir()  # Every attempt is new; never overwrite old data or evidence.
    expected = {}
    for version, (relative, sha) in ARCHIVES.items():
        archive = (Path.home() / 'Metrix-Writer/Releases') / relative
        assert digest(archive) == sha
        with tarfile.open(archive) as tar:
            for member in tar:
                p = Path(member.name)
                assert not p.is_absolute() and '..' not in p.parts
                assert member.isfile() or member.isdir(), 'Archive links/devices are not admitted'
        output = root / 'expected' / version
        output.mkdir(parents=True)
        run(['tar', '-xzf', str(archive), '-C', str(output)])
        apps = list(output.glob('*.app'))
        assert len(apps) == 1
        expected[version] = app_identity(apps[0])
        expected[version]['archive_sha256'] = sha
    (root / 'install').mkdir()
    app = next((root / 'expected' / baseline).glob('*.app'))
    run(['cp', '-cR', str(app), str(root / 'install' / app.name)])
    source = repo / '.local/native-20261011/same-identity/profile'
    profile = root / 'profile'
    profile.mkdir()
    assert (source / 'Data').is_dir() and (source / 'Models').is_dir()
    run(['cp', '-cR', str(source / 'Models'), str(profile / 'Models')])
    if baseline == '0.19.0':
        # The newer job format prevents this legacy client from initializing.
        # Preserve every copied byte as a sentinel; use an empty legacy store.
        (profile / 'Data').mkdir()
        run(['cp', '-cR', str(source / 'Data'), str(profile / 'Data/acceptance-saved')])
    else:
        run(['cp', '-cR', str(source / 'Data'), str(profile / 'Data')])
    (profile / 'State').mkdir()
    for name in ['onboarding.json', 'settings.json', 'system.json', 'models.json']:
        if (source / 'State' / name).is_file():
            (profile / 'State' / name).write_bytes((source / 'State' / name).read_bytes())
    # New baseline profile has no prior updater floor or credential/account store.
    assert not (profile / 'State/update-release.json').exists()
    (profile / 'State/tauri-preferences.json').write_text(json.dumps({'recording': 'Ctrl+Alt+R', 'toggle': False}) + '\n')
    (profile / 'saved-audio.fixture').write_bytes(b'RIFF independent saved-audio sentinel\0')
    (profile / 'raw-transcript.fixture').write_text('Independent raw text retained across genuine GUI upgrades.\n')
    (root / 'home').mkdir()
    policy = '(version 1)\n(allow default)\n'
    for name in ['WebKit', 'Caches']:
        policy += '(deny file-read* file-write* (subpath ' + json.dumps(str(Path.home() / 'Library' / name / 'com.metrix.writer')) + '))\n'
    (root / 'browser-boundary.sb').write_text(policy)
    (root / '.fixture').write_text('MW-SEC-29 private GUI update chain\n')
    write(root, 'expected-identities', expected)
    write(root, 'before-private', inventory(profile))
    write(root, 'prepared', {'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                            'baseline': baseline, 'private_profile_files': len(inventory(profile)),
                            'copied_data_role': 'retained sentinels; empty legacy native store' if baseline == '0.19.0' else 'native saved store',
                            'live_installation_or_keys_modified': False, 'gui_update_chain': 'not run'})
    print('Prepared exact private baseline, retained saved audio/jobs/models and browser boundary; no app launched.')


def owned_pid(root):
    app = next((root / 'install').glob('*.app'))
    needle = str(app / 'Contents/MacOS/metrix-writer-desktop')
    pids = []
    for line in run(['ps', '-axo', 'pid=,command=']).decode().splitlines():
        if needle in line and '--user-root ' + str(root / 'profile') in line and not any(x in line for x in ['sandbox-exec', '/usr/bin/env']):
            pids.append(int(line.strip().split(None, 1)[0]))
    assert len(pids) == 1, 'Private app absent or ambiguous'
    return pids[0], app


def launch(root):
    app = next((root / 'install').glob('*.app'))
    label = 'mw-sec-29-gui-chain-' + root.name
    assert subprocess.run(['launchctl', 'print', f'gui/{os.getuid()}/{label}'], capture_output=True).returncode != 0
    command = ['/usr/bin/env']
    for name in ['OPENAI_API_KEY', 'GEMINI_API_KEY', 'XAI_API_KEY', 'OPENROUTER_API_KEY',
                 'DYLD_INSERT_LIBRARIES', 'DYLD_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH']:
        command += ['-u', name]
    command += ['CFFIXED_USER_HOME=' + str(root / 'home'), '__XPC_CFFIXED_USER_HOME=' + str(root / 'home'),
                '/usr/bin/sandbox-exec', '-f', str(root / 'browser-boundary.sb'),
                str(app / 'Contents/MacOS/metrix-writer-desktop'), '--user-root', str(root / 'profile')]
    plist = root / 'launch.plist'
    assert not plist.exists()
    plist.write_bytes(plistlib.dumps({'Label': label, 'ProgramArguments': command,
                                     'RunAtLoad': True, 'KeepAlive': False,
                                     # Tauri spawns the replacement before exiting.
                                     # launchd must let that owned child survive.
                                     'AbandonProcessGroup': True,
                                     'StandardOutPath': str(root / 'app.log'),
                                     'StandardErrorPath': str(root / 'app.error.log')}))
    plist.chmod(0o600)
    run(['launchctl', 'bootstrap', f'gui/{os.getuid()}', str(plist)])
    write(root, 'launch', {'label': label, 'submitted_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                          'keep_alive': False, 'abandon_process_group': True,
                          'private_identity': app_identity(app), 'actual_running_ui': 'pending verification'})
    print('Private native GUI launch submitted; not update acceptance.')


def observe(root, phase):
    pid, app = owned_pid(root)
    identity = app_identity(app)
    expected = json.loads((root / 'expected-identities.json').read_text())[identity['version']]
    assert identity['executable_sha256'] == expected['executable_sha256']
    actual = inventory(root / 'profile')
    before = json.loads((root / 'before-private.json').read_text())
    saved = {k: v for k, v in before.items() if k.startswith(('Data/', 'Models/')) or k.endswith('.fixture')}
    missing = sum(actual.get(k) != v for k, v in saved.items())
    write(root, phase + '-private', actual)
    write(root, phase, {'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                       'pid': pid, 'identity': identity, 'original_saved_files': len(saved),
                       'saved_files_missing_or_changed': missing, 'native_restart_retains_private_root': True,
                       'limits': ['Running version and saved files only; actual UI steps/observations require separate records.']})
    assert missing == 0, 'Saved data changed; inspect private retained maps'
    print(json.dumps({'pid': pid, 'version': identity['version'], 'saved_files': len(saved), 'missing_or_changed': missing}))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('mode', choices=['prepare', 'launch', 'observe'])
    p.add_argument('--root', type=Path, required=True)
    p.add_argument('--baseline', choices=['0.19.0', '0.37.0'], default='0.19.0')
    p.add_argument('--phase', default='baseline')
    args = p.parse_args()
    assert platform.system() == 'Darwin' and platform.machine() == 'arm64'
    repo = Path(__file__).resolve().parents[3]
    root = args.root.resolve()
    assert root.parent == repo / '.local/native-20261012-gui-chain' and root.name in ['legacy', 'same-identity']
    root.parent.mkdir(exist_ok=True)
    if args.mode == 'prepare':
        prepare(repo, root, args.baseline)
    else:
        assert (root / '.fixture').read_text() == 'MW-SEC-29 private GUI update chain\n'
        if args.mode == 'launch':
            launch(root)
        else:
            assert args.phase.replace('-', '').isalnum()
            observe(root, args.phase)


if __name__ == '__main__':
    main()
