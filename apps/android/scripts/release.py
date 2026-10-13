#!/usr/bin/env python3
"""Build Writer's Android releases with source, TLS, package and signer receipts.

`build` makes the maintainer's private self-updating dev APK (com.metrix.writer). `play-bundle`
makes the Google Play AAB (com.tricktic.dictate), signed by the Play upload key.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import tarfile
import tempfile
import io
import zipfile

import dev

ROOT = dev.ROOT
KEYS = Path(os.environ.get('WRITER_ANDROID_KEYS',
    Path.home() / '.local/share/metrix-writer/android-release-signing'))
DEV_PACKAGE = 'com.metrix.writer'
PLAY_PACKAGE = 'com.tricktic.dictate'
PLAY_KEYS = Path(os.environ.get('WRITER_ANDROID_PLAY_KEYS',
    Path.home() / '.local/share/metrix-writer/android-play-upload'))
PLAY_ALIAS = 'dictate-upload'
# Tracked public fingerprint of the registered Play upload certificate.
PLAY_PIN = ROOT / 'play-upload-cert.sha256'
PLAY_TAG = 'android-play-v'
# Self-update code, install permission and sideload wording must never reach the Play build.
PLAY_FORBIDDEN = (b'Lcom/metrix/writer/AndroidUpdater;', b'Lcom/metrix/writer/UpdateTransfer;',
                  b'Lcom/metrix/writer/InstallReceiver;', b'REQUEST_INSTALL_PACKAGES',
                  b'/writer/channels/android', b'without Google Play', b'Allow restricted settings',
                  # MW-RELEASE-12: remove these exclusions only with recorded acceptance of
                  # account-enabled Data safety/deletion declarations and qualified Play sign-in.
                  # A merged documentation PR or a submitted form is not acceptance.
                  b'Lcom/google/firebase/auth/', b'Landroidx/credentials/',
                  b'Lcom/google/android/libraries/identity/googleid/',
                  b'Lcom/metrix/writer/account/FirebaseGoogleIdentity;')


def inspect_play_dex(bundle):
    """Reject disallowed capabilities in every AAB module before publishing a bundle."""
    for name in bundle.namelist():
        if '/dex/' in name and name.endswith('.dex'):
            dex = bundle.read(name)
            if any(marker in dex for marker in PLAY_FORBIDDEN):
                raise ValueError(f'Unapproved sign-in, self-update code or sideload wording in Play {name}')


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, capture_output=True, **kwargs).stdout.decode()


def external(path):
    path = path.expanduser().resolve()
    parent = path
    while not parent.exists():
        parent = parent.parent
    check = subprocess.run(['git', '-C', str(parent), 'rev-parse', '--is-inside-work-tree'], capture_output=True)
    if check.returncode == 0 and check.stdout.strip() == b'true':
        raise ValueError('Signing and artifact directories must be outside source checkouts')
    return path


def init_keys(directory):
    directory = external(directory)
    # Never replace a key: a different signer cannot update an installed private app.
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    password = secrets.token_hex(32)
    env = {**os.environ, 'WRITER_SIGNING_PASSWORD': password}
    key = directory / 'writer.p12'
    run(['keytool', '-genkeypair', '-keystore', key, '-storetype', 'PKCS12',
         '-storepass:env', 'WRITER_SIGNING_PASSWORD', '-keypass:env', 'WRITER_SIGNING_PASSWORD',
         '-alias', 'writer-private', '-keyalg', 'RSA', '-keysize', '3072', '-validity', '10950',
         '-dname', 'CN=Writer Private Android', '-noprompt'], env=env)
    key.chmod(0o600)
    # java.util.Properties path escaping; generated passwords contain hex only.
    escaped = str(key).replace('\\', '\\\\').replace(':', '\\:').replace(' ', '\\ ')
    config = directory / 'signing.properties'
    config.write_text(f'storeFile={escaped}\nstorePassword={password}\nkeyAlias=writer-private\nkeyPassword={password}\n')
    config.chmod(0o600)
    run(['keytool', '-exportcert', '-keystore', key, '-storepass:env', 'WRITER_SIGNING_PASSWORD',
         '-alias', 'writer-private', '-file', directory / 'signer.der'], env=env)
    print(f'Created persistent private signer in {directory}. Back up this directory securely before delivery.')


def inspect(apk, *, allow_legacy_fixture=False, package_name=DEV_PACKAGE):
    run([dev.tool('build-tools/36.0.0/zipalign'), '-c', '-P', '16', '4', apk])
    signer = run([dev.tool('build-tools/36.0.0/apksigner'), 'verify', '--verbose', '--print-certs', apk])
    digest = re.search(r'Signer #1 certificate SHA-256 digest: ([0-9a-f]{64})', signer)
    if not digest or 'Signer #2' in signer:
        raise ValueError('Expected one verified APK signer')
    badging = run([dev.tool('build-tools/36.0.0/aapt2'), 'dump', 'badging', apk])
    package = re.search(r"package: name='([^']+)' versionCode='([0-9]+)' versionName='([^']+)'", badging)
    if not package or package[1] != package_name or 'application-debuggable' in badging:
        raise ValueError('Wrong identity or debuggable release')
    if "minSdkVersion:'33'" not in badging or "targetSdkVersion:'36'" not in badging:
        raise ValueError('Unexpected release Android floor or target')
    if package_name == PLAY_PACKAGE and 'REQUEST_INSTALL_PACKAGES' in badging:
        raise ValueError('The Play build must not request package installation')
    manifest = run([dev.tool('build-tools/36.0.0/aapt2'), 'dump', 'xmltree', apk, '--file', 'AndroidManifest.xml'])
    if 'E: instrumentation' in manifest or 'testOnly' in manifest or 'debuggable' in manifest and 'debuggable(0x0101000f)=false' not in manifest:
        raise ValueError('Test instrumentation or debug flag in release manifest')
    resources = run([dev.tool('build-tools/36.0.0/aapt2'), 'dump', 'resources', apk])
    network = re.search(r'resource (\S+) xml/network_security_config\s+\([^\n]*\) \(file\) (\S+)', resources)
    if not network or ('networkSecurityConfig(0x01010527)=@' + network[1]) not in manifest:
        raise ValueError('Network policy resource missing')
    policy = run([dev.tool('build-tools/36.0.0/aapt2'), 'dump', 'xmltree', apk, '--file', network[2]])
    if 'cleartextTrafficPermitted=false' not in policy or any(x in policy for x in ('domain-config', 'trust-anchors', 'debug-overrides')):
        raise ValueError('Release must use system TLS trust only, without cleartext exceptions')
    with zipfile.ZipFile(apk) as archive:
        if any('writer_test_ca' in name for name in archive.namelist()):
            raise ValueError('Test CA in release')
        delivery_fixture = False
        for name in archive.namelist():
            if name.endswith('.dex') and any(marker in archive.read(name) for marker in
                    (b'queued_fixture', b'Delivery-only fixture in 3 seconds. No recording.')):
                delivery_fixture = True
        if delivery_fixture and not allow_legacy_fixture:
            raise ValueError('Automatic editor-delivery fixture in release code')
    return {'package': package[1], 'version_code': int(package[2]), 'version_name': package[3],
            'apk_sha256': hashlib.sha256(Path(apk).read_bytes()).hexdigest(),
            'signer_sha256': digest[1], 'debuggable': False, 'tls': 'system trust; no cleartext exceptions',
            'automatic_delivery_fixture': delivery_fixture}


def build(args):
    keys = external(args.keys)
    for name in ('signing.properties', 'writer.p12', 'signer.der'):
        if not (keys / name).is_file():
            raise ValueError('Missing persistent signer. Run init-key once, or restore the original key backup.')
    for name in ('signing.properties', 'writer.p12'):
        if (keys / name).stat().st_mode & 0o077:
            raise ValueError('Signing secrets must be owner-readable only (chmod 600)')
    status = run(['git', '-C', ROOT, 'status', '--porcelain'])
    source = run(['git', '-C', ROOT, 'rev-parse', 'HEAD']).strip()
    if status.strip() and not args.allow_dirty:
        raise ValueError('Commit the source before a release; --allow-dirty is for development evidence only')
    output = external(args.output)
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    env = dict(os.environ)
    env.pop('WRITER_ANDROID_TEST_CA', None)
    env['WRITER_ANDROID_SIGNING_PROPERTIES'] = str(keys / 'signing.properties')
    command = [str(ROOT / 'gradlew'), '--no-daemon', '--offline', ':app:assembleDevRelease', ':app:lintDevRelease']
    if args.version_code:
        command += [f'-PwriterVersionCode={args.version_code}']
    if args.version_name:
        command += [f'-PwriterVersionName={args.version_name}']
    if args.with_upgrade_test:
        command += ['-PwriterReleaseTest=true', '-PwriterTestRunner=com.metrix.writer.UpgradeRunner', ':app:assembleDevReleaseAndroidTest']
    with (output / 'build.log').open('w') as log:
        subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    if source != run(['git', '-C', ROOT, 'rev-parse', 'HEAD']).strip() or status != run(['git', '-C', ROOT, 'status', '--porcelain']):
        raise ValueError('Source changed during build; commit it and build a new candidate')
    apk = output / 'writer.apk'
    apk.write_bytes((ROOT / 'app/build/outputs/apk/dev/release/app-dev-release.apk').read_bytes())
    receipt = inspect(apk)
    expected = hashlib.sha256((keys / 'signer.der').read_bytes()).hexdigest()
    if receipt['signer_sha256'] != expected:
        raise ValueError('Signer differs from the retained original certificate')
    receipt.update(source_commit=source,
                   source_tree=run(['git', '-C', ROOT, 'rev-parse', 'HEAD^{tree}']).strip(),
                   source_dirty=bool(status.strip()), build_command=command,
                   built_at_utc=run(['date', '-u', '+%Y-%m-%dT%H:%M:%SZ']).strip())
    if args.with_upgrade_test:
        test = output / 'upgrade-test.apk'
        test.write_bytes((ROOT / 'app/build/outputs/apk/androidTest/dev/release/app-dev-release-androidTest.apk').read_bytes())
        test_signer = run([dev.tool('build-tools/36.0.0/apksigner'), 'verify', '--print-certs', test])
        if f'certificate SHA-256 digest: {expected}' not in test_signer:
            raise ValueError('Upgrade instrumentation must have the same signer')
        receipt['test_apk_sha256'] = hashlib.sha256(test.read_bytes()).hexdigest()
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    (output / 'SHA256SUMS').write_text(f"{receipt['apk_sha256']}  writer.apk\n")
    print(json.dumps({'directory': str(output), **receipt}, indent=2))


def signing_values(keys):
    values = {}
    for line in (keys / 'signing.properties').read_text().splitlines():
        if '=' in line and not line.startswith('#'):
            name, value = line.split('=', 1)
            values[name] = value.replace('\\:', ':').replace('\\ ', ' ').replace('\\\\', '\\')
    return values


def create_key(directory, alias, name, der_name, key_name):
    """A new PKCS12 key with a generated password in owner-only files; never replaces one."""
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    password = secrets.token_hex(32)
    env = {**os.environ, 'WRITER_SIGNING_PASSWORD': password}
    key = directory / key_name
    run(['keytool', '-genkeypair', '-keystore', key, '-storetype', 'PKCS12',
         '-storepass:env', 'WRITER_SIGNING_PASSWORD', '-keypass:env', 'WRITER_SIGNING_PASSWORD',
         '-alias', alias, '-keyalg', 'RSA', '-keysize', '4096', '-validity', '10950',
         '-dname', f'CN={name}', '-noprompt'], env=env)
    key.chmod(0o600)
    escaped = str(key).replace('\\', '\\\\').replace(':', '\\:').replace(' ', '\\ ')
    config = directory / 'signing.properties'
    config.write_text(f'storeFile={escaped}\nstorePassword={password}\nkeyAlias={alias}\nkeyPassword={password}\n')
    config.chmod(0o600)
    run(['keytool', '-exportcert', '-keystore', key, '-storepass:env', 'WRITER_SIGNING_PASSWORD',
         '-alias', alias, '-file', directory / der_name], env=env)
    return hashlib.sha256((directory / der_name).read_bytes()).hexdigest()


def gpg(home, args, secret, data):
    """Run gpg in a private home, passing the passphrase on a pipe (never argv or env)."""
    read, write = os.pipe()
    os.write(write, secret.encode() + b'\n'); os.close(write)
    try:
        return subprocess.run(['gpg', '--homedir', home, '--batch', '--quiet', '--no-tty', '--pinentry-mode', 'loopback',
                               '--passphrase-fd', str(read), *args], input=data, pass_fds=(read,),
                              check=True, capture_output=True).stdout
    finally:
        os.close(read)
        subprocess.run(['gpgconf', '--homedir', home, '--kill', 'gpg-agent'], capture_output=True)


def restore_drill(keys, archive, passphrase):
    """Decrypt the backup, compare every byte and prove the restored key signs as the pinned certificate."""
    with tempfile.TemporaryDirectory(prefix='writer-play-restore-') as temp:
        temp = Path(temp); home = temp / 'gnupg'; home.mkdir(mode=0o700)
        plain = gpg(home, ['--decrypt'], passphrase, archive.read_bytes())
        restored = temp / 'restored'
        with tarfile.open(fileobj=io.BytesIO(plain)) as bundle:
            names = sorted(m.name for m in bundle.getmembers() if m.isfile())
            if names != ['signing.properties', 'upload.der', 'upload.p12']:
                raise ValueError('Backup does not contain exactly the Play upload key files')
            for name in names:
                if bundle.extractfile(name).read() != (keys / name).read_bytes():
                    raise ValueError(f'Restored {name} differs from the live key')
            bundle.extractall(restored, filter='data')
        values = signing_values(restored)
        password = values['storePassword']
        env = {**os.environ, 'WRITER_SIGNING_PASSWORD': password}
        fixture = temp / 'restore-challenge.jar'
        with zipfile.ZipFile(fixture, 'w') as jar: jar.writestr('challenge.txt', secrets.token_hex(16))
        run(['jarsigner', '-keystore', restored / 'upload.p12', '-storetype', 'PKCS12',
             '-storepass:env', 'WRITER_SIGNING_PASSWORD', '-keypass:env', 'WRITER_SIGNING_PASSWORD',
             fixture, values['keyAlias']], env=env)
        verified = run(['jarsigner', '-verify', fixture])
        digest = certificate_sha256(run(['keytool', '-printcert', '-jarfile', fixture]))
        expected = hashlib.sha256((keys / 'upload.der').read_bytes()).hexdigest()
        if 'jar verified' not in verified or digest != expected:
            raise ValueError('Restored key does not sign as the live upload certificate')
        return digest


def certificate_sha256(printcert):
    if 'Signer #2' in printcert:
        raise ValueError('Expected exactly one signer')
    found = re.findall(r'SHA256: ([0-9A-F:]{95})', printcert)
    if len(found) != 1:
        raise ValueError('Expected exactly one signing certificate')
    return found[0].replace(':', '').lower()


def play_init_key(args):
    keys, backup, handoff = external(args.keys), external(args.backup), external(args.handoff)
    if handoff.exists() or backup.exists():
        raise ValueError('Refusing to replace an existing backup or passphrase handoff file')
    digest = create_key(keys, PLAY_ALIAS, 'TrickTic Dictate Play Upload', 'upload.der', 'upload.p12')
    backup.mkdir(mode=0o700, parents=True)
    passphrase = secrets.token_urlsafe(32)
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w') as bundle:
        for name in ('signing.properties', 'upload.der', 'upload.p12'):
            bundle.add(keys / name, arcname=name)
    archive = backup / 'play-upload-key.tar.gpg'
    with tempfile.TemporaryDirectory(prefix='writer-play-backup-') as home:
        archive.write_bytes(gpg(home, ['--symmetric', '--cipher-algo', 'AES256', '--s2k-digest-algo', 'SHA512',
                                       '--s2k-count', '65011712', '--output', '-'], passphrase, buffer.getvalue()))
    handoff.write_text(passphrase + '\n'); handoff.chmod(0o600)
    restored = restore_drill(keys, archive, passphrase)
    (backup / 'SHA256SUMS').write_text(f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n')
    (backup / 'README.txt').write_text(
        'TrickTic Dictate Google Play upload key: GPG AES-256 encrypted archive of the key directory.\n'
        f'Upload certificate SHA-256: {digest}\n'
        'The passphrase is in the owner password manager, never next to this archive.\n'
        'Restore drill: python3 apps/android/scripts/release.py play-verify-backup --backup <this directory>\n')
    print(json.dumps({'keys': str(keys), 'backup': str(archive), 'upload_cert_sha256': digest,
                      'restore_drill': 'passed' if restored == digest else 'failed',
                      'passphrase_handoff': str(handoff)}, indent=2))
    print(f'Track the public fingerprint in {PLAY_PIN.relative_to(ROOT.parent.parent)}. Move the passphrase from the '
          'handoff file into the password manager, then delete that file.')


def play_verify_backup(args):
    import getpass
    passphrase = Path(args.passphrase_file).read_text().strip() if args.passphrase_file else getpass.getpass('Backup passphrase: ')
    digest = restore_drill(external(args.keys), external(args.backup) / 'play-upload-key.tar.gpg', passphrase)
    print(json.dumps({'restore_drill': 'passed', 'upload_cert_sha256': digest}, indent=2))


def play_version():
    text = (ROOT / 'gradle.properties').read_text()
    version = re.search(r'^writerPlayVersionName=(\d+)\.(\d{1,3})\.(\d{1,3})$', text, re.M)
    if not version:
        raise ValueError('writerPlayVersionName must be MAJOR.MINOR.PATCH in apps/android/gradle.properties')
    major, minor, patch = (int(part) for part in version.groups())
    return f'{major}.{minor}.{patch}', major * 1_000_000 + minor * 1_000 + patch, (major, minor, patch)


def play_provenance(version, parts):
    """Play consumes a versionCode even for a refused upload: build only a tagged, clean, increasing release."""
    if run(['git', '-C', ROOT, 'status', '--porcelain', '--untracked-files=normal']).strip():
        raise ValueError('Refusing a Play bundle from a dirty working tree')
    branch = subprocess.run(['git', '-C', str(ROOT), 'symbolic-ref', '--quiet', '--short', 'HEAD'],
                            capture_output=True).stdout.decode().strip() or 'detached'
    if branch != 'main' and not branch.startswith('release/') and branch != 'detached':
        raise ValueError(f"Refusing a Play bundle from '{branch}'; use main, release/* or a detached release tag")
    tag = PLAY_TAG + version
    if tag not in run(['git', '-C', ROOT, 'tag', '--points-at', 'HEAD']).split():
        raise ValueError(f'Play source must be tagged {tag} at HEAD')
    for other in run(['git', '-C', ROOT, 'tag', '--list', PLAY_TAG + '*']).split():
        match = re.fullmatch(re.escape(PLAY_TAG) + r'(\d+)\.(\d+)\.(\d+)', other)
        if other != tag and match and tuple(int(x) for x in match.groups()) >= parts:
            raise ValueError(f'{other} already exists; the Play version must increase')
    return branch, tag


def bundletool(classpath, *args):
    result = subprocess.run(['java', 'com.android.tools.build.bundletool.BundleToolMain', *(str(a) for a in args)],
                            env={**os.environ, 'CLASSPATH': classpath}, capture_output=True)
    if result.returncode:
        raise ValueError(f'bundletool {args[0]} failed: ' + result.stderr.decode()[-2000:])
    return result.stdout.decode()


def inspect_bundle(aab, checks, keys, version, code, classpath):
    """Verify signature, identity, alignment and approved capabilities before any upload."""
    checks.mkdir(mode=0o700)
    verified = run(['jarsigner', '-verify', '-verbose', '-certs', aab])
    (checks / 'jarsigner-verify.txt').write_text(verified)
    if 'jar verified.' not in verified:
        raise ValueError('jarsigner did not verify the Play bundle')
    printcert = run(['keytool', '-printcert', '-jarfile', aab])
    (checks / 'upload-certificate.txt').write_text(printcert)
    upload = certificate_sha256(printcert)
    manifest = bundletool(classpath, 'dump', 'manifest', '--bundle', aab)
    (checks / 'AndroidManifest.xml').write_text(manifest)
    expected = (f'package="{PLAY_PACKAGE}"', f'android:versionCode="{code}"', f'android:versionName="{version}"',
                'android:minSdkVersion="33"', 'android:targetSdkVersion="36"')
    if not all(value in manifest for value in expected):
        raise ValueError('Play bundle identity, version or SDK levels differ from the tracked release')
    if any(value in manifest for value in ('REQUEST_INSTALL_PACKAGES', 'com.metrix.writer.InstallReceiver', 'android:debuggable="true"', 'android:testOnly')):
        raise ValueError('Play manifest contains install, update, debug or test entries')
    config = bundletool(classpath, 'dump', 'config', '--bundle', aab)
    (checks / 'BundleConfig.json').write_text(config)
    if 'PAGE_ALIGNMENT_16K' not in config:
        raise ValueError('Play bundle is not configured for 16 KB page alignment')
    readelf = dev.tool('ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf')
    alignment, report = {}, []
    with zipfile.ZipFile(aab) as bundle, tempfile.TemporaryDirectory(prefix='writer-play-elf-') as temp:
        inspect_play_dex(bundle)
        for name in bundle.namelist():
            if re.fullmatch(r'base/lib/[^/]+/[^/]+\.so', name):
                library = Path(temp) / name.replace('/', '_')
                library.write_bytes(bundle.read(name))
                headers = run([readelf, '-lW', library])
                report.append(f'== {name}\n{headers}')
                loads = [int(line.split()[-1], 16) for line in headers.splitlines() if line.strip().startswith('LOAD')]
                if not loads or min(loads) < 0x4000:
                    raise ValueError(f'{name} has an ELF LOAD segment aligned below 16 KB')
                alignment[name] = hex(min(loads))
    (checks / 'elf-load-alignment.txt').write_text('\n'.join(report))
    if {name.split('/')[2] for name in alignment} != {'arm64-v8a', 'x86_64'}:
        raise ValueError('Play bundle must contain arm64-v8a and x86_64 native code')
    # The Play-served split APKs come from this bundle; a universal APK exercises the same release checks.
    values = signing_values(keys)
    password = checks / '.signing-password'
    try:
        password.write_text(values['storePassword'])
        bundletool(classpath, 'build-apks', '--bundle', aab, '--output', checks / 'universal.apks', '--mode', 'universal',
                   '--aapt2', dev.tool('build-tools/36.0.0/aapt2'),
                   '--ks', keys / 'upload.p12', '--ks-key-alias', values['keyAlias'],
                   '--ks-pass', f'file:{password}', '--key-pass', f'file:{password}')
    finally:
        password.unlink(missing_ok=True)
    with zipfile.ZipFile(checks / 'universal.apks') as apks:
        (checks / 'universal.apk').write_bytes(apks.read('universal.apk'))
    (checks / 'universal.apks').unlink()
    apk = inspect(checks / 'universal.apk', package_name=PLAY_PACKAGE)
    if apk['automatic_delivery_fixture'] or apk['version_code'] != code:
        raise ValueError('Universal APK from the Play bundle failed release inspection')
    return {'upload_cert_sha256': upload, 'page_alignment': 'PAGE_ALIGNMENT_16K', 'elf_load_alignment': alignment,
            'forbidden_markers_absent': [marker.decode() for marker in PLAY_FORBIDDEN],
            'universal_apk_sha256': apk['apk_sha256'], 'tls': apk['tls'], 'debuggable': False}


def play_bundle(args):
    version, code, parts = play_version()
    dry = args.dry_run_test_key
    source = run(['git', '-C', ROOT, 'rev-parse', 'HEAD']).strip()
    status = run(['git', '-C', ROOT, 'status', '--porcelain', '--untracked-files=normal'])
    branch, tag = ('dry-run', None) if dry else play_provenance(version, parts)
    pin = None
    if not dry:
        pin = next((line.strip().lower() for line in PLAY_PIN.read_text().splitlines()
                    if line.strip() and not line.startswith('#')), '')
        if not re.fullmatch(r'[0-9a-f]{64}', pin):
            raise ValueError(f'{PLAY_PIN.name} must hold the registered upload certificate SHA-256')
    output = external(args.output)
    if output.exists():
        raise ValueError('Choose a new output directory; existing Play artifacts are never replaced')
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f'.{output.name}.tmp-', dir=output.parent))
    temporary = tempfile.TemporaryDirectory(prefix='writer-play-dry-run-key-') if dry else None
    try:
        if dry:
            keys = Path(temporary.name) / 'keys'
            create_key(keys, 'dictate-dry-run', 'TrickTic Dictate Play Dry Run Not For Upload', 'upload.der', 'upload.p12')
        else:
            keys = external(args.keys)
            for name in ('signing.properties', 'upload.p12', 'upload.der'):
                if not (keys / name).is_file():
                    raise ValueError('Missing Play upload key. Restore it from its encrypted backup; never generate a replacement.')
                if (keys / name).stat().st_mode & 0o077:
                    raise ValueError('Upload key files must be owner-readable only (chmod 600)')
            if hashlib.sha256((keys / 'upload.der').read_bytes()).hexdigest() != pin:
                raise ValueError(f'Refusing to build: {keys} is not the registered upload key in {PLAY_PIN.name}')
        env = dict(os.environ)
        for name in ('WRITER_ANDROID_TEST_CA', 'WRITER_ANDROID_SIGNING_PROPERTIES'):
            env.pop(name, None)
        env['WRITER_ANDROID_PLAY_SIGNING_PROPERTIES'] = str(keys / 'signing.properties')
        command = [str(ROOT / 'gradlew'), '--no-daemon', '--offline', ':app:bundlePlayRelease', ':app:lintPlayRelease']
        with (staging / 'build.log').open('w') as log:
            subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        if source != run(['git', '-C', ROOT, 'rev-parse', 'HEAD']).strip() or status != run(['git', '-C', ROOT, 'status', '--porcelain', '--untracked-files=normal']):
            raise ValueError('Source changed during build; commit it and build a new candidate')
        classpath = run([str(ROOT / 'gradlew'), '--offline', '-q', ':app:printBundletoolClasspath'], cwd=ROOT).strip()
        aab = staging / f'dictate-{version}.aab'
        shutil.copyfile(ROOT / 'app/build/outputs/bundle/playRelease/app-play-release.aab', aab)
        checks = inspect_bundle(aab, staging / 'checks', keys, version, code, classpath)
        expected = pin or hashlib.sha256((keys / 'upload.der').read_bytes()).hexdigest()
        if checks['upload_cert_sha256'] != expected:
            raise ValueError('Refusing the bundle: its upload certificate does not match '
                             + (PLAY_PIN.name if pin else 'the dry-run key'))
        digest = hashlib.sha256(aab.read_bytes()).hexdigest()
        receipt = {'package': PLAY_PACKAGE, 'version_name': version, 'version_code': code,
                   'channel': 'play-dry-run-test-key' if dry else 'play', 'aab': aab.name, 'aab_sha256': digest,
                   'aab_bytes': aab.stat().st_size, 'pinned_upload_cert_sha256': pin, **checks,
                   'source_commit': source, 'source_tree': run(['git', '-C', ROOT, 'rev-parse', 'HEAD^{tree}']).strip(),
                   'source_branch': branch, 'source_tag': tag, 'source_dirty': bool(status.strip()),
                   'bundletool': bundletool(classpath, 'version').strip(), 'build_command': command,
                   'built_at_utc': run(['date', '-u', '+%Y-%m-%dT%H:%M:%SZ']).strip()}
        (staging / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        (staging / 'SHA256SUMS').write_text(f'{digest}  {aab.name}\n')
        (staging / 'README.txt').write_text(
            ('DRY RUN: signed by a throwaway key. Never upload anything from this directory.\n' if dry else
             f'Upload only {aab.name} to the intended Play Console track, after checking receipt.json:\n'
             'channel "play", the version, the source tag, and upload_cert_sha256 equal to the registered upload key.\n')
            + 'checks/ holds the verification output and a universal APK for emulator testing, never for distribution.\n')
        os.rename(staging, output)
        staging = None
        print(json.dumps({'directory': str(output), **receipt}, indent=2))
    finally:
        if staging is not None:
            shutil.rmtree(staging, ignore_errors=True)
        if temporary is not None:
            temporary.cleanup()


def main():
    os.umask(0o077)
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest='command', required=True)
    init = sub.add_parser('init-key'); init.add_argument('--keys', type=Path, default=KEYS)
    b = sub.add_parser('build')
    b.add_argument('--keys', type=Path, default=KEYS)
    b.add_argument('--output', type=Path, required=True)
    b.add_argument('--version-code', type=int)
    b.add_argument('--version-name')
    b.add_argument('--with-upgrade-test', action='store_true')
    b.add_argument('--allow-dirty', action='store_true')
    check = sub.add_parser('inspect'); check.add_argument('apk', type=Path)
    check.add_argument('--package', default=DEV_PACKAGE, choices=[DEV_PACKAGE, PLAY_PACKAGE])
    play_key = sub.add_parser('play-init-key', help='Create the Play upload key with an encrypted, restore-tested backup')
    play_key.add_argument('--keys', type=Path, default=PLAY_KEYS)
    play_key.add_argument('--backup', type=Path, required=True)
    play_key.add_argument('--handoff', type=Path, required=True, help='Owner-only file receiving the backup passphrase')
    drill = sub.add_parser('play-verify-backup', help='Restore drill for the Play upload key backup')
    drill.add_argument('--keys', type=Path, default=PLAY_KEYS)
    drill.add_argument('--backup', type=Path, required=True)
    drill.add_argument('--passphrase-file', type=Path)
    bundle = sub.add_parser('play-bundle', help='Build and verify the signed Google Play AAB')
    bundle.add_argument('--keys', type=Path, default=PLAY_KEYS)
    bundle.add_argument('--output', type=Path, required=True)
    bundle.add_argument('--dry-run-test-key', action='store_true', help='Throwaway key; waives provenance. Never upload.')
    args = p.parse_args()
    if args.command == 'init-key': init_keys(args.keys)
    elif args.command == 'build': build(args)
    elif args.command == 'play-init-key': play_init_key(args)
    elif args.command == 'play-verify-backup': play_verify_backup(args)
    elif args.command == 'play-bundle': play_bundle(args)
    else: print(json.dumps(inspect(args.apk, package_name=args.package), indent=2))


if __name__ == '__main__':
    main()
