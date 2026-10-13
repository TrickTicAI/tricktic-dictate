#!/usr/bin/env python3
"""MW-SEC-23 public Play build gate: artifact, SBOM, emulator and phone evidence.

`artifact` inspects the exact AAB and, once Play has served it, the Play-signed split APKs.
`sbom` binds the AAB to the MW-SEC-16 release scan. `emulator` runs the destructive and
fixture-signed checks on an owned emulator. `phone` records read-only checks of a Play-installed
build beside the dev build. Nothing here uploads, promotes, signs with a production key or
changes a release channel.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile

import dev
import release

ROOT = dev.ROOT
REPO = ROOT.parent.parent
PLAY = release.PLAY_PACKAGE
DEV = release.DEV_PACKAGE
# Tracked public fingerprint of Google's app-signing certificate, recorded from Play Console (MW-RELEASE-9).
APP_SIGNING_PIN = ROOT / 'play-app-signing-cert.sha256'
# The card's dependencies: the tested source must contain each one's recorded commit.
DEPENDENCIES = ('MW-SEC-8', 'MW-SEC-9', 'MW-SEC-12', 'MW-SEC-13', 'MW-SEC-16', 'MW-SEC-17', 'MW-SEC-18', 'MW-SEC-20')
# Reviewed manifest surface. A new permission or exported component needs a new security review.
PERMISSIONS = {'android.permission.RECORD_AUDIO', 'android.permission.FOREGROUND_SERVICE',
               'android.permission.FOREGROUND_SERVICE_MICROPHONE', 'android.permission.POST_NOTIFICATIONS',
               'android.permission.SYSTEM_ALERT_WINDOW', 'android.permission.HIDE_OVERLAY_WINDOWS',
               'android.permission.INTERNET', 'android.permission.ACCESS_NETWORK_STATE',
               f'{PLAY}.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'}
EXPORTED = {'com.metrix.writer.MainActivity': None, 'com.metrix.writer.FloatingSetupActivity': None,
            'com.metrix.writer.WriterInputService': 'android.permission.BIND_ACCESSIBILITY_SERVICE',
            'com.metrix.writer.TranscriptionJobService': 'android.permission.BIND_JOB_SERVICE',
            'androidx.profileinstaller.ProfileInstallReceiver': 'android.permission.DUMP'}
FIXTURE_MARKERS = (b'queued_fixture', b'Delivery-only fixture in 3 seconds. No recording.')
# MW-SEC-9's model-loader guards compile these messages into libwriter-phone.so.
GUARDS = (b'truncated model scalar', b'invalid model dimensions', b'invalid mel filter dimensions')
WHISPER = '927cfce34f31707e17f2bff35c349632fb9e2c3a'
A = '{http://schemas.android.com/apk/res/android}'
BUILD_TOOLS = 'build-tools/36.0.0'


def sha256(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def run(args, **kwargs):
    return release.run(args, **kwargs)


class Checks:
    """Named pass/fail/pending results; a pending item is never reported as a pass."""
    def __init__(self):
        self.items = []

    def add(self, name, ok, detail=None, pending=False):
        status = 'pending' if pending else ('pass' if ok else 'fail')
        self.items.append({'check': name, 'status': status, **({'detail': detail} if detail is not None else {})})
        print(f'[{status}] {name}' + (f': {detail}' if isinstance(detail, str) else ''))
        return ok

    def guard(self, name, action):
        try:
            return self.add(name, True, action())
        except Exception as error:  # A failed check is evidence, not a crash.
            return self.add(name, False, str(error)[-1500:])

    def summary(self):
        counts = {s: sum(i['status'] == s for i in self.items) for s in ('pass', 'fail', 'pending')}
        return counts


def pin(path):
    if not path.exists():
        return None
    value = next((line.strip().lower() for line in path.read_text().splitlines() if line.strip() and not line.startswith('#')), '')
    if not re.fullmatch(r'[0-9a-f]{64}', value):
        raise ValueError(f'{path.name} must hold one SHA-256 fingerprint')
    return value


def board_commit(card):
    text = run([REPO / 'scripts/orchestration/board.sh', 'show', card])
    status = re.search(r'^status: "?([a-z_]+)"?$', text, re.M)
    commit = re.search(r'^commit: "?([0-9a-f]{40})"?$', text, re.M)
    if not status or status[1] != 'merged' or not commit:
        raise ValueError(f'{card} is not merged with a recorded commit')
    return commit[1]


def ancestry(source):
    result = {}
    for card in DEPENDENCIES:
        commit = board_commit(card)
        check = subprocess.run(['git', '-C', str(REPO), 'merge-base', '--is-ancestor', commit, source], capture_output=True)
        result[card] = {'commit': commit, 'contained': check.returncode == 0}
    missing = [card for card, item in result.items() if not item['contained']]
    if missing:
        raise ValueError('Source lacks merged dependencies: ' + ', '.join(f'{c} {result[c]["commit"][:8]}' for c in missing))
    return result


def manifest_facts(xml):
    root = ET.fromstring(xml)
    app = root.find('application')
    sdk = root.find('uses-sdk')
    exported = {}
    for node in app:
        if node.tag in ('activity', 'activity-alias', 'service', 'receiver', 'provider'):
            name = node.get(A + 'name')
            flag = node.get(A + 'exported')
            if flag == 'true' or (flag is None and node.find('intent-filter') is not None):
                exported[name] = node.get(A + 'permission')
    return {'package': root.get('package'), 'version_code': int(root.get(A + 'versionCode')),
            'version_name': root.get(A + 'versionName'),
            'min_sdk': int(sdk.get(A + 'minSdkVersion')), 'target_sdk': int(sdk.get(A + 'targetSdkVersion')),
            'debuggable': app.get(A + 'debuggable'), 'test_only': app.get(A + 'testOnly'),
            'allow_backup': app.get(A + 'allowBackup'), 'data_extraction_rules': app.get(A + 'dataExtractionRules'),
            'network_security_config': app.get(A + 'networkSecurityConfig'),
            'uses_cleartext': app.get(A + 'usesCleartextTraffic'),
            'permissions': sorted(p.get(A + 'name') for p in root.findall('uses-permission')),
            'exported': exported,
            'authorities': sorted(p.get(A + 'authorities') for p in app.findall('provider'))}


def check_manifest(facts, version_code=None):
    problems = []
    if facts['package'] != PLAY: problems.append('package ' + str(facts['package']))
    if version_code and facts['version_code'] != version_code: problems.append('versionCode differs from build receipt')
    if (facts['min_sdk'], facts['target_sdk']) != (33, 36): problems.append('SDK levels')
    if facts['debuggable'] == 'true' or facts['test_only']: problems.append('debuggable/testOnly')
    if facts['allow_backup'] != 'false' or not facts['data_extraction_rules']: problems.append('backup policy')
    if not facts['network_security_config'] or facts['uses_cleartext'] == 'true': problems.append('network policy')
    extra = set(facts['permissions']) - PERMISSIONS
    if extra: problems.append('unreviewed permissions ' + ', '.join(sorted(extra)))
    if facts['exported'] != EXPORTED: problems.append(f'exported components differ: {facts["exported"]}')
    if any(not a.startswith(PLAY + '.') for a in facts['authorities']): problems.append('provider authority outside the Play package')
    if problems:
        raise ValueError('; '.join(problems))
    return facts


def elf(tools, path):
    headers = run([tools, '-lW', path])
    loads = [int(line.split()[-1], 16) for line in headers.splitlines() if line.strip().startswith('LOAD')]
    needed = re.findall(r'\(NEEDED\)\s+Shared library: \[([^\]]+)\]', run([tools, '-dW', path]))
    return min(loads) if loads else 0, sorted(needed)


def maven(group, name, extension):
    """Locked release version and its Gradle-verified cached file; None when not cached locally."""
    lock = (ROOT / 'app/gradle.lockfile').read_text()
    version = re.search(rf'^{re.escape(group)}:{re.escape(name)}:([^=]+)=.*releaseRuntimeClasspath', lock, re.M)
    if not version:
        raise ValueError(f'{group}:{name} is not locked on releaseRuntimeClasspath')
    cache = Path.home() / '.gradle/caches/modules-2/files-2.1' / group / name / version[1]
    found = sorted(cache.glob(f'*/{name}-{version[1]}.{extension}'))
    return version[1], (found[0] if found else None)


def androidx_graphics_path():
    """The AAR that ships libandroidx.graphics.path.so."""
    return maven('androidx.graphics', 'graphics-path', 'aar')


def native_inventory(bundle, temp):
    readelf = dev.tool('ndk/28.2.13676358/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf')
    graphics_version, aar = androidx_graphics_path()
    upstream = {}
    if aar:
        with zipfile.ZipFile(aar) as archive:
            for name in archive.namelist():
                if name.startswith('jni/') and name.endswith('.so'):
                    upstream[name.split('/')[1]] = hashlib.sha256(archive.read(name)).hexdigest()
    libraries, problems = [], []
    for name in sorted(bundle.namelist()):
        match = re.fullmatch(r'base/lib/([^/]+)/([^/]+\.so)', name)
        if not match:
            continue
        data = bundle.read(name)
        path = Path(temp) / name.replace('/', '_')
        path.write_bytes(data)
        align, needed = elf(readelf, path)
        item = {'path': name, 'abi': match[1], 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
                'elf_load_alignment': hex(align), 'needed': needed}
        if align < 0x4000:
            problems.append(f'{name} LOAD alignment {hex(align)}')
        if match[2] == 'libwriter-phone.so':
            item['component'] = f'whisper.cpp {WHISPER} + Writer JNI'
            item['mw_sec_9_guards'] = all(guard in data for guard in GUARDS)
            if not item['mw_sec_9_guards']:
                problems.append(f'{name} lacks the MW-SEC-9 model-loader guards')
        elif match[2] == 'libandroidx.graphics.path.so':
            item['component'] = f'androidx.graphics:graphics-path:{graphics_version}'
            item['matches_verified_aar'] = upstream.get(match[1]) == item['sha256'] if aar else None
            if aar and not item['matches_verified_aar']:
                problems.append(f'{name} differs from the Gradle-verified AAR')
        else:
            problems.append(f'{name} has no reviewed component')
        libraries.append(item)
    if {item['abi'] for item in libraries} != {'arm64-v8a', 'x86_64'}:
        problems.append('expected arm64-v8a and x86_64 native code')
    return libraries, problems


def throwaway_key(directory):
    """Only to open the AAB as APKs locally; these APKs are never distributed."""
    release.create_key(directory, 'gate-inspection', 'TrickTic Dictate gate inspection only', 'upload.der', 'upload.p12')
    return directory


def build_apks(classpath, aab, output, keys, *extra):
    """Sign locally generated APKs with the fixture/throwaway key described by keys/signing.properties."""
    values = release.signing_values(keys)
    password = keys / '.password'
    try:
        password.write_text(values['storePassword'])
        release.bundletool(classpath, 'build-apks', '--bundle', aab, '--output', output, '--aapt2', dev.tool(BUILD_TOOLS + '/aapt2'),
                           '--ks', values['storeFile'], '--ks-key-alias', values['keyAlias'],
                           '--ks-pass', f'file:{password}', '--key-pass', f'file:{password}', *extra)
    finally:
        password.unlink(missing_ok=True)


def apk_signer(apk):
    verified = run([dev.tool(BUILD_TOOLS + '/apksigner'), 'verify', '--verbose', '--print-certs', apk])
    digests = re.findall(r'Signer #\d+ certificate SHA-256 digest: ([0-9a-f]{64})', verified)
    if len(digests) != 1:
        raise ValueError(f'{Path(apk).name}: expected exactly one verified signer')
    return digests[0]


def play_apks(checks, directory, aab_libraries, upload, version_code, output):
    """Inspect the split APKs Play delivered (pulled from a Play install or Console download)."""
    apks = sorted(Path(directory).glob('*.apk'))
    base = [apk for apk in apks if apk.name == 'base.apk']
    if len(base) != 1:
        return checks.add('Play-delivered APK set', False, 'base.apk missing')
    expected = pin(APP_SIGNING_PIN)
    signers = {}
    for apk in apks:
        checks.guard(f'{apk.name}: 16 KB zip alignment', lambda apk=apk: run(
            [dev.tool(BUILD_TOOLS + '/zipalign'), '-c', '-P', '16', '4', apk]) and 'aligned')
        signers[apk.name] = apk_signer(apk)
    record = {'apks': {apk.name: {'sha256': sha256(apk), 'signer_sha256': signers[apk.name]} for apk in apks}}
    distinct = set(signers.values())
    checks.add('Play APKs share one signer, not the upload key', len(distinct) == 1 and upload not in distinct,
               {'signer_sha256': sorted(distinct)})
    if expected:
        checks.add('Play app-signing certificate equals the tracked pin', distinct == {expected}, expected)
    else:
        checks.add('Play app-signing certificate pin', False, f'{APP_SIGNING_PIN.name} not yet recorded from Play Console', pending=True)
    checks.guard('Play base APK release inspection', lambda: release.inspect(base[0], package_name=PLAY))
    badging = run([dev.tool(BUILD_TOOLS + '/aapt2'), 'dump', 'badging', base[0]])
    checks.add('Play APK version equals the AAB', f"versionCode='{version_code}'" in badging, version_code)
    by_abi = {(item['abi'], Path(item['path']).name): item['sha256'] for item in aab_libraries}
    served, forbidden = {}, []
    for apk in apks:
        with zipfile.ZipFile(apk) as archive:
            for name in archive.namelist():
                if name.endswith('.dex') and any(m in archive.read(name) for m in release.PLAY_FORBIDDEN + FIXTURE_MARKERS):
                    forbidden.append(f'{apk.name}:{name}')
                match = re.fullmatch(r'lib/([^/]+)/([^/]+\.so)', name)
                if match:
                    served[match[1], match[2]] = hashlib.sha256(archive.read(name)).hexdigest()
    checks.add('Play APKs contain no updater, sideload or fixture code', not forbidden, forbidden or None)
    mismatched = [f'{abi}/{lib}' for (abi, lib), digest in served.items() if by_abi.get((abi, lib)) != digest]
    checks.add('Play-served native libraries are byte-identical to the AAB', bool(served) and not mismatched,
               {'served': sorted(f'{a}/{l}' for a, l in served), 'mismatched': mismatched})
    (output / 'play-apks.json').write_text(json.dumps(record, indent=2) + '\n')


def artifact(args):
    output = release.external(args.out)
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    checks = Checks()
    aab = args.aab.expanduser().resolve()
    digest = sha256(aab)
    receipt = json.loads(args.build_receipt.read_text()) if args.build_receipt else {}
    evidence = {'aab': str(aab), 'aab_sha256': digest, 'aab_bytes': aab.stat().st_size,
                'rehearsal_test_key': args.test_key}
    if receipt:
        checks.add('AAB hash equals the build receipt', receipt.get('aab_sha256') == digest, receipt.get('aab_sha256'))
        checks.add('Build receipt: clean tagged source', not receipt.get('source_dirty') and bool(receipt.get('source_tag')),
                   {k: receipt.get(k) for k in ('source_commit', 'source_tag', 'source_dirty', 'channel')},
                   pending=args.test_key)
    source = args.source_commit or receipt.get('source_commit')
    evidence['source_commit'] = source
    if source:
        checks.guard('Source contains every merged dependency of MW-SEC-23', lambda: ancestry(source))
    else:
        checks.add('Source contains every merged dependency of MW-SEC-23', False, 'no source commit supplied')
    printcert = run(['keytool', '-printcert', '-jarfile', aab])
    upload = release.certificate_sha256(printcert)
    checks.add('jarsigner verifies the AAB', 'jar verified.' in run(['jarsigner', '-verify', aab]))
    expected_upload = pin(release.PLAY_PIN)
    if args.test_key:
        checks.add('Upload certificate is the registered pin', False, 'rehearsal: throwaway key', pending=True)
    else:
        checks.add('Upload certificate is the registered pin', upload == expected_upload, upload)
    evidence['upload_cert_sha256'] = upload
    classpath = run([ROOT / 'gradlew', '--offline', '-q', ':app:printBundletoolClasspath'], cwd=ROOT).strip()
    xml = release.bundletool(classpath, 'dump', 'manifest', '--bundle', aab)
    (output / 'AndroidManifest.xml').write_text(xml)
    facts = manifest_facts(xml)
    evidence['manifest'] = facts
    checks.guard('Manifest identity, SDK, backup, network, permissions and exported components',
                 lambda: check_manifest(facts, receipt.get('version_code')) and 'reviewed surface')
    config = release.bundletool(classpath, 'dump', 'config', '--bundle', aab)
    checks.add('Bundle requests 16 KB page alignment', 'PAGE_ALIGNMENT_16K' in config)
    with zipfile.ZipFile(aab) as bundle, tempfile.TemporaryDirectory(prefix='play-gate-') as temp:
        markers = [name for name in bundle.namelist() if name.startswith('base/dex/')
                   and any(m in bundle.read(name) for m in release.PLAY_FORBIDDEN + FIXTURE_MARKERS)]
        checks.add('No updater, install permission, sideload wording or delivery fixture in dex', not markers, markers or None)
        checks.add('No test CA resource', not any('writer_test_ca' in n for n in bundle.namelist()))
        libraries, problems = native_inventory(bundle, temp)
        evidence['native_libraries'] = libraries
        checks.add('Native library inventory: 16 KB LOAD alignment, reviewed components, MW-SEC-9 guards',
                   not problems, problems or f'{len(libraries)} libraries')
        keys = throwaway_key(Path(temp) / 'keys')
        apks = Path(temp) / 'universal.apks'
        build_apks(classpath, aab, apks, keys, '--mode', 'universal')
        with zipfile.ZipFile(apks) as archive:
            (Path(temp) / 'universal.apk').write_bytes(archive.read('universal.apk'))
        inspected = checks.guard('APK from the AAB: system TLS only, not debuggable, no fixture, 16 KB zip',
                                 lambda: release.inspect(Path(temp) / 'universal.apk', package_name=PLAY))
    if args.play_apks:
        play_apks(checks, args.play_apks, libraries, upload, facts['version_code'], output)
    else:
        checks.add('Play-delivered APKs (Play app signing)', False, 'no Play install or Console download yet', pending=True)
    counts = checks.summary()
    passed = counts['fail'] == 0 and inspected
    evidence.update(checks=checks.items, counts=counts,
                    artifact_result='pass' if passed and not counts['pending'] else ('incomplete' if passed else 'fail'))
    (output / 'receipt.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps({'directory': str(output), 'artifact_result': evidence['artifact_result'], **counts}, indent=2))
    return 0 if passed else 1


def sbom(args):
    """Stage the AAB for scripts/security/gate.py release; run from a clean checkout of the build commit."""
    receipt = json.loads(args.build_receipt.read_text())
    head = run(['git', '-C', REPO, 'rev-parse', 'HEAD']).strip()
    if receipt['source_commit'] != head:
        raise ValueError(f'Run from a checkout of the build commit {receipt["source_commit"]}, not {head}')
    output = args.out.expanduser().resolve()
    if not output.is_relative_to(REPO / '.local'):
        raise ValueError('Stage under this checkout\'s ignored .local/ directory, as the release gate requires')
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    aab = output / args.aab.name
    shutil.copyfile(args.aab, aab)
    if sha256(aab) != receipt['aab_sha256']:
        raise ValueError('AAB differs from its build receipt')
    payload = output / 'payload'
    with zipfile.ZipFile(aab) as bundle:
        for info in bundle.infolist():
            target = (payload / info.filename).resolve()
            if not target.is_relative_to(payload.resolve()) or (info.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError(f'Unsafe entry {info.filename}')
        bundle.extractall(payload)
    manifest = output / 'manifest.json'
    subprocess.run(['python3', REPO / 'scripts/security/manifest.py', '--platform', 'android', '--payload', payload,
                    '--artifact', aab, '--out', manifest], check=True)
    data = json.loads(manifest.read_text())
    graphics_version, aar = androidx_graphics_path()
    if not aar:
        raise ValueError('graphics-path AAR is not in the Gradle cache; build once offline first')
    # Play's crash-symbol metadata is derived from the same library; it is not delivered to devices.
    writer = sorted(n for n in data['files'] if n.endswith(('/libwriter-phone.so', '/libwriter-phone.so.sym')))
    graphics = sorted(n for n in data['files'] if n.endswith('/libandroidx.graphics.path.so'))
    # A JVM class resource whose CAFEBABE magic the native scanner reads as a Mach-O fat header.
    coroutines_version, jar = maven('org.jetbrains.kotlinx', 'kotlinx-coroutines-core-jvm', 'jar')
    probes = 'base/root/DebugProbesKt.bin'
    if probes in data['files']:
        with zipfile.ZipFile(jar) as archive:
            if hashlib.sha256(archive.read('DebugProbesKt.bin')).hexdigest() != data['files'][probes]['sha256']:
                raise ValueError('DebugProbesKt.bin differs from the Gradle-verified coroutines jar')
    whisper = {'url': 'https://github.com/ggml-org/whisper.cpp', 'revision': WHISPER}
    data.update(
        builder=f'release.py play-bundle on the release host; NDK 28.2.13676358; receipt built_at_utc {receipt["built_at_utc"]}',
        extraction_method='Python zipfile extraction of the signed AAB (play-gate.py sbom); no code executed',
        components=[
            {'name': 'ggml-org/whisper.cpp', 'version': '1.9.4', 'source': whisper, 'osv_query': {'commit': WHISPER}, 'files': writer},
            {'name': 'ggml-org/ggml (vendored in whisper.cpp)', 'version': '1.9.4', 'source': whisper,
             'osv_query': {'commit': WHISPER}, 'files': writer},
            {'name': 'TrickTicAI/tricktic-dictate phone JNI', 'version': receipt['version_name'],
             'source': {'url': 'https://github.com/TrickTicAI/tricktic-dictate', 'revision': head},
             'osv_query': {'commit': head}, 'files': writer},
            # androidx publishes no source commit for this prebuilt; the revision is the Gradle-verified AAR hash.
            {'name': 'androidx.graphics:graphics-path', 'version': graphics_version,
             'source': {'url': 'https://android.googlesource.com/platform/frameworks/support', 'revision': sha256(aar)},
             'osv_query': {'package': {'ecosystem': 'Maven', 'name': 'androidx.graphics:graphics-path'}, 'version': graphics_version},
             'files': graphics},
            {'name': 'org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm', 'version': coroutines_version,
             'source': {'url': 'https://github.com/Kotlin/kotlinx.coroutines', 'revision': sha256(jar)},
             'osv_query': {'package': {'ecosystem': 'Maven', 'name': 'org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm'},
                           'version': coroutines_version},
             'files': [probes] if probes in data['files'] else []}])
    data['components'] = [c for c in data['components'] if c['files']]
    manifest.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')
    result = subprocess.run(['python3', REPO / 'scripts/security/gate.py', 'release', '--manifest', manifest,
                             '--out', output / 'release-receipt.json', '--sbom', output / 'sbom.cdx.json'], cwd=REPO)
    print(f'Security release gate exit {result.returncode}; receipt and SBOM in {output}')
    return result.returncode


# ---------------------------------------------------------------- emulator

def emulator(args):
    """Fixture-signed Play release beside a differently signed dev release on an owned emulator."""
    if not args.serial.startswith('emulator-'):
        raise ValueError('Destructive gate fixtures run only on an owned emulator')
    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    output = release.external(args.directory)
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    adb = dev.tool('platform-tools/adb')
    checks = Checks()

    def log(label, command, **kwargs):
        result = subprocess.run([str(x) for x in command], cwd=ROOT, capture_output=True, timeout=kwargs.pop('timeout', 1800), **kwargs)
        (output / f'{label}.log').write_bytes(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(f'{label} failed; see {output}/{label}.log')
        return result.stdout.decode(errors='replace')

    def shell(*command):
        return dev.adb(args.serial, 'shell', *command).decode(errors='replace')

    signing = dev.PRIVATE / 'play-gate-signing'
    signing.mkdir(mode=0o700, exist_ok=True)
    keys = {}
    for name in ('play', 'dev', 'attacker'):
        directory = signing / name
        if not directory.exists():
            release.create_key(directory, 'fixture', f'Writer play gate {name} fixture', 'cert.der', 'key.p12')
        keys[name] = directory
    ca = signing / 'test-ca.pem'
    if not ca.exists():
        values = release.signing_values(keys['attacker'])
        ca.write_text(run(['keytool', '-exportcert', '-rfc', '-keystore', keys['attacker'] / 'key.p12',
                           '-storepass', values['storePassword'], '-alias', 'fixture']))
    env = {k: v for k, v in os.environ.items() if not k.startswith('WRITER_ANDROID_') or k in ('WRITER_ANDROID_DEV', 'WRITER_ANDROID_API')}
    env.update(WRITER_ANDROID_PLAY_SIGNING_PROPERTIES=str(keys['play'] / 'signing.properties'),
               WRITER_ANDROID_SIGNING_PROPERTIES=str(keys['dev'] / 'signing.properties'),
               WRITER_ANDROID_TEST_CA=str(ca))
    version, code, (major, minor, patch) = release.play_version()
    skipped = f'{major}.{minor}.{patch + 2}'
    common = ['--offline', '-PwriterReleaseTest=true', '-PwriterTestRunner=androidx.test.runner.AndroidJUnitRunner']
    log('build-v1', [ROOT / 'gradlew', *common, ':app:bundlePlayRelease', ':app:assemblePlayReleaseAndroidTest',
                     ':app:assembleDevRelease', ':editor:assembleDebug'], env=env)
    builds = output / 'builds'
    builds.mkdir()
    shutil.copyfile(ROOT / 'app/build/outputs/bundle/playRelease/app-play-release.aab', builds / f'play-{version}.aab')
    shutil.copyfile(ROOT / 'app/build/outputs/apk/androidTest/play/release/app-play-release-androidTest.apk', builds / 'play-test.apk')
    shutil.copyfile(ROOT / 'app/build/outputs/apk/dev/release/app-dev-release.apk', builds / 'dev-release.apk')
    values = release.signing_values(keys['attacker'])
    log('sign-attacker', [dev.tool(BUILD_TOOLS + '/apksigner'), 'sign', '--ks', keys['attacker'] / 'key.p12',
                          '--ks-pass', 'pass:' + values['storePassword'], '--out', builds / 'hostile-editor.apk',
                          ROOT / 'editor/build/outputs/apk/debug/editor-debug.apk'])
    log('build-v2', [ROOT / 'gradlew', *common, f'-PwriterPlayVersionName={skipped}', ':app:bundlePlayRelease'], env=env)
    shutil.copyfile(ROOT / 'app/build/outputs/bundle/playRelease/app-play-release.aab', builds / f'play-{skipped}.aab')

    classpath = run([ROOT / 'gradlew', '--offline', '-q', ':app:printBundletoolClasspath'], cwd=ROOT).strip()
    for name in (version, skipped):
        build_apks(classpath, builds / f'play-{name}.aab', builds / f'play-{name}.apks', keys['play'],
                   '--connected-device', '--device-id', args.serial, '--adb', adb)
    identities = {}
    for name in ('dev-release.apk', 'play-test.apk', 'hostile-editor.apk'):
        identities[name] = {'sha256': sha256(builds / name), 'signer_sha256': apk_signer(builds / name)}
    for name in (version, skipped):
        identities[f'play-{name}.aab'] = {'sha256': sha256(builds / f'play-{name}.aab')}
    signers = {identities[n]['signer_sha256'] for n in ('dev-release.apk', 'play-test.apk', 'hostile-editor.apk')}
    checks.add('Play, dev and attacker fixtures have three distinct signers', len(signers) == 3)

    installed = shell('pm', 'list', 'packages').split()
    for package in (PLAY, DEV, 'com.metrix.writer.editor', PLAY + '.test'):
        if 'package:' + package in installed:
            raise ValueError(f'{package} is already installed; use a fresh owned emulator')
    try:
        log('install-dev', [adb, '-s', args.serial, 'install', '--no-incremental', builds / 'dev-release.apk'])
        shell('am', 'start', '-W', '-n', f'{DEV}/com.metrix.writer.MainActivity')
        time.sleep(2); shell('am', 'force-stop', DEV)
        release.bundletool(classpath, 'install-apks', '--apks', builds / f'play-{version}.apks', '--device-id', args.serial, '--adb', adb)
        log('install-test', [adb, '-s', args.serial, 'install', '--no-incremental', builds / 'play-test.apk'])
        log('install-attacker', [adb, '-s', args.serial, 'install', '--no-incremental', builds / 'hostile-editor.apk'])
        paths = shell('pm', 'path', PLAY).split()
        checks.add('Play build installed as split APKs from the AAB', len(paths) > 1, paths)

        runner = f'{PLAY}.test/androidx.test.runner.AndroidJUnitRunner'
        canary = 'sk-gate-canary-' + hashlib.sha256(os.urandom(16)).hexdigest()[:24]
        shell('logcat', '-c')

        def instrument(label, selection, *extra, expect=None):
            out = log(label, [adb, '-s', args.serial, 'shell', 'am', 'instrument', '-w', '-e', 'class', selection,
                              '-e', 'canary', canary, *extra, runner])
            ok = 'FAILURES' not in out and 'OK (' in out and (expect is None or f'OK ({expect} test' in out)
            return checks.add(label, ok, re.findall(r'OK \(\d+ tests?\)|Tests run: \d+,  Failures: \d+', out))

        security = ','.join('com.metrix.writer.' + name for name in (
            'AndroidBoundaryTest', 'AndroidStorageSecurityTest', 'ProtectedWindowTest',
            'WriterUiTest#cloudKeyEntryIsProtectedAndNeverRestoredWithActivityState',
            'WriterUiTest#dictionaryCreateEditAndDeleteWithHebrewSnippet'))
        instrument('separate-UID boundaries, Keystore, backup and network policy, protected windows', security, expect=10)
        instrument('history deletion erases export snapshots and caches', 'com.metrix.writer.CacheDeletionTest')
        gate = 'com.metrix.writer.ReleaseGateTest#'
        instrument('provider key absent from logs and private storage', gate + 'testProviderKeyAbsentFromLogsAndPlainStorage', expect=1)
        instrument('Play build cannot reach dev build files, components or exports', gate + 'testOtherBuildDataIsInaccessible',
                   '-e', 'other', DEV, expect=1)
        instrument('storage exhaustion retains saved audio', gate + 'testStorageExhaustionRetainsSavedAudio', expect=1)

        # Recording in flight while a newer build skipping one version replaces the package, as a Play update does.
        phase = subprocess.Popen([adb, '-s', args.serial, 'shell', 'am', 'instrument', '-w', '-e', 'class',
                                  gate + 'testSeedAndRecordUntilPackageReplaced', '-e', 'canary', canary, runner],
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        deadline = time.time() + 120
        while 'recording-in-flight' not in shell('logcat', '-d', '-s', 'WriterGate:I'):
            if time.time() > deadline or phase.poll() is not None:
                raise RuntimeError('In-flight recording did not start')
            time.sleep(1)
        release.bundletool(classpath, 'install-apks', '--apks', builds / f'play-{skipped}.apks', '--device-id', args.serial, '--adb', adb)
        phase_output = phase.communicate(timeout=120)[0].decode(errors='replace')
        (output / 'in-flight-phase-1.log').write_text(phase_output)
        checks.add('Package replaced while recording (phase 1 process ended by the update)', 'OK (' not in phase_output)
        dumped = shell('dumpsys', 'package', PLAY)
        checks.add(f'Updated {version} -> {skipped}, skipping one version', f'versionName={skipped}' in dumped)
        instrument('saved audio, raw text and key survive a skipped-version update; in-flight capture recovered',
                   gate + 'testRecoveredAfterSkippedVersionUpdate', expect=1)

        backup = shell('bmgr', 'backupnow', PLAY)
        (output / 'bmgr-backupnow.txt').write_text(backup)
        checks.add('Backup manager does not back up the Play build', 'Backup is not allowed' in backup, backup.strip()[-300:])
        packages = {name: shell('dumpsys', 'package', name) for name in (PLAY, DEV)}
        for name, text in packages.items():
            (output / f'dumpsys-{name}.txt').write_text(text)
        uid = {name: re.search(r'(?:appId|userId)=(\d+)', text)[1] for name, text in packages.items()}
        checks.add('Play and dev builds have distinct UIDs and no shared user', uid[PLAY] != uid[DEV]
                   and not any('sharedUser=' in t for t in packages.values()), uid)
        checks.add('Play build flags exclude debuggable and backup',
                   not re.search(r'pkgFlags=\[[^\]]*(DEBUGGABLE|ALLOW_BACKUP)', packages[PLAY]))
        logcat = shell('logcat', '-d')
        (output / 'logcat-sizes.txt').write_text(f'{len(logcat)} characters scanned; content not retained\n')
        needles = (canary, 'synthetic-key-never-contact-a-provider', 'synthetic-replacement-never-contact-provider',
                   'ui-fixture-never-send')
        checks.add('No synthetic provider key in the device log', not any(n in logcat for n in needles))

        counts = checks.summary()
        receipt = {'serial': args.serial, 'owned_emulator_pid': owner['pid'], 'physical_phone': False,
                   'sdk': shell('getprop', 'ro.build.version.sdk').strip(),
                   'fingerprint': shell('getprop', 'ro.build.fingerprint').strip(),
                   'commit': run(['git', '-C', REPO, 'rev-parse', 'HEAD']).strip(),
                   'dirty': bool(run(['git', '-C', REPO, 'status', '--porcelain']).strip()),
                   'versions': [version, skipped], 'artifacts': identities, 'uids': uid,
                   'signers': 'disposable fixtures only; never the upload or Play app-signing key',
                   'checks': checks.items, 'counts': counts, 'passed': counts['fail'] == 0}
        (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print(json.dumps({'directory': str(output), **counts}, indent=2))
    finally:
        for package in (PLAY + '.test', PLAY, DEV, 'com.metrix.writer.editor'):
            subprocess.run([adb, '-s', args.serial, 'uninstall', package], capture_output=True)
    return 0 if receipt['passed'] else 1


# ------------------------------------------------------------------- phone

def phone(args):
    """Read-only checks of the Play-installed build on a physical phone, beside the dev build."""
    dev.selected(args.serial)
    output = release.external(args.directory)
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    checks = Checks()

    def shell(*command):
        return dev.adb(args.serial, 'shell', *command).decode(errors='replace')

    packages = {}
    for name in (PLAY, DEV):
        text = shell('dumpsys', 'package', name)
        (output / f'dumpsys-{name}.txt').write_text(text)
        packages[name] = text
    installer = re.search(r'installerPackageName=(\S+)', packages[PLAY])
    checks.add('Play build installed by Google Play', bool(installer) and installer[1] == 'com.android.vending',
               installer[1] if installer else None)
    uid = {name: (re.search(r'(?:appId|userId)=(\d+)', text) or [None, None])[1] for name, text in packages.items()}
    checks.add('Dev build installed beside it with a distinct UID', all(uid.values()) and uid[PLAY] != uid[DEV], uid)
    checks.add('Play build is not debuggable and does not allow backup',
               not re.search(r'pkgFlags=\[[^\]]*(DEBUGGABLE|ALLOW_BACKUP)', packages[PLAY]))
    pulled = output / 'play-apks'
    pulled.mkdir()
    for line in shell('pm', 'path', PLAY).split():
        remote = line.removeprefix('package:')
        dev.adb(args.serial, 'pull', remote, str(pulled / Path(remote).name))
    checks.add('Pulled the Play-delivered APK set', (pulled / 'base.apk').exists(), sorted(p.name for p in pulled.iterdir()))
    backup = shell('bmgr', 'backupnow', PLAY)
    (output / 'bmgr-backupnow.txt').write_text(backup)
    checks.add('Backup manager does not back up the Play build', 'Backup is not allowed' in backup, backup.strip()[-300:])
    if args.hostile:
        # A separately signed, permission-free app attacks the store-signed build; it logs its own results.
        shell('logcat', '-c')
        shell('am', 'start', '-W', '-n', 'com.metrix.writer.editor/.HostileActivity', '--es', 'target', PLAY,
              '--es', 'operation', 'components')
        time.sleep(3)
        lines = [l for l in shell('logcat', '-d', '-s', 'WriterHostile:I').splitlines() if '{' in l]
        result = json.loads(lines[-1][lines[-1].index('{'):]) if lines else {}
        denied = [k for k, v in result.items() if k != 'uid' and v == 'denied']
        checks.add('Separate UID is denied every private component of the Play build',
                   len(result) == 15 and len(denied) == 14, result)
    if args.canary:
        logcat = shell('logcat', '-d')
        checks.add('Canary key typed into the Play build is absent from the device log', args.canary not in logcat)
    counts = checks.summary()
    receipt = {'serial': args.serial, 'physical_phone': True, 'model': shell('getprop', 'ro.product.model').strip(),
               'fingerprint': shell('getprop', 'ro.build.fingerprint').strip(), 'uids': uid,
               'checks': checks.items, 'counts': counts, 'passed': counts['fail'] == 0,
               'next': f'play-gate.py artifact --play-apks {pulled} ...'}
    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'directory': str(output), **counts}, indent=2))
    return 0 if receipt['passed'] else 1


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    a = sub.add_parser('artifact', help='Inspect the AAB and any Play-delivered APKs')
    a.add_argument('--aab', type=Path, required=True)
    a.add_argument('--build-receipt', type=Path, help='receipt.json from release.py play-bundle')
    a.add_argument('--source-commit', help='when no build receipt is supplied')
    a.add_argument('--play-apks', type=Path, help='directory of base.apk and split_*.apk served by Play')
    a.add_argument('--test-key', action='store_true', help='rehearsal on a dry-run bundle; can never pass the gate')
    a.add_argument('--out', type=Path, required=True)
    s = sub.add_parser('sbom', help='Run the MW-SEC-16 release scan and SBOM on the AAB')
    s.add_argument('--aab', type=Path, required=True)
    s.add_argument('--build-receipt', type=Path, required=True)
    s.add_argument('--out', type=Path, required=True, help='new directory under this checkout\'s .local/')
    e = sub.add_parser('emulator', help='Fixture-signed gate checks on an owned emulator')
    e.add_argument('--serial', required=True)
    e.add_argument('--directory', type=Path, required=True)
    p = sub.add_parser('phone', help='Read-only checks of a Play-installed build on a phone')
    p.add_argument('--serial', required=True)
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--hostile', action='store_true', help='the independently signed editor fixture is installed')
    p.add_argument('--canary', help='synthetic key the operator typed into the Play build')
    args = parser.parse_args()
    raise SystemExit({'artifact': artifact, 'sbom': sbom, 'emulator': emulator, 'phone': phone}[args.command](args))


if __name__ == '__main__':
    main()
