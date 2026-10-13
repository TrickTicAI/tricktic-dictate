import datetime as dt
import hashlib
import os
import json
from pathlib import Path
import subprocess
import argparse
import contextlib
import io
import zipfile
import tarfile
import base64
import plistlib
import struct

import gate
import tempfile
import unittest
from unittest.mock import patch

from artifacts import release_input
from inventory import digest, inventory, maven, pnpm, sbom
from scanning import (ScanError, binary_strings, osv, payload_allowances, policy_findings, sanitized_secrets,
                      secrets)
from public_metadata import signature_regions, resource_regions

ROOT = Path(__file__).resolve().parents[2]


def sample():
    return {'ecosystem': 'npm', 'name': 'synthetic-package', 'version': '1.0.0',
            'purl': 'pkg:npm/synthetic-package@1.0.0', 'scopes': ['production'], 'evidence': ['fixture.lock']}


def finding():
    return {'id': 'TEST-001', 'purl': sample()['purl'], 'severity': 'high', 'modified': '2026-09-20T00:00:00Z'}


class AdvisoryTests(unittest.TestCase):
    def test_clean_and_vulnerable_online_fixtures(self):
        clean, database = osv([sample()], lambda path, data=None: {'results': [{}]})
        self.assertEqual(clean, [])
        self.assertEqual(database['packages_queried'], 1)
        self.assertIn('queried_at', database)
        def fetch(path, data=None):
            if path == 'querybatch':
                return {'results': [{'vulns': [{'id': 'TEST-001', 'modified': '2026-09-20'}]}]}
            return {'id': 'TEST-001', 'modified': '2026-09-20', 'database_specific': {'severity': 'HIGH'}}
        findings, database = osv([sample()], fetch)
        self.assertEqual(len(policy_findings(findings, {'schema_version': 1, 'exceptions': []}, 'web')), 1)
        self.assertEqual(database['record_modified_at'], {'TEST-001': '2026-09-20'})

    def test_empty_short_or_invalid_response_fails_closed(self):
        for response in ({}, {'results': []}, {'results': [{'error': 'failed'}]}):
            with self.subTest(response=response), self.assertRaises(ScanError):
                osv([sample()], lambda path, data=None: response)

    def test_pagination_keeps_later_vulnerability(self):
        calls = []
        def fetch(path, data=None):
            calls.append((path, data))
            if path != 'querybatch':
                return {'id': 'TEST-002', 'modified': '2026-09-20'}
            if 'page_token' not in data['queries'][0]:
                return {'results': [{'next_page_token': 'second'}]}
            return {'results': [{'vulns': [{'id': 'TEST-002', 'modified': '2026-09-20'}]}]}
        findings, _ = osv([sample()], fetch)
        self.assertEqual(findings[0]['id'], 'TEST-002')
        self.assertEqual(findings[0]['severity'], 'unknown')
        self.assertEqual(len(calls), 3)

    def test_exception_exact_expiring_and_advisory_bound(self):
        exception = {**finding(), 'platform': 'web', 'reason': 'Synthetic unreachable test path',
                     'reviewer': 'fixture-reviewer', 'disposition': 'not-reachable', 'expires': '2026-10-01'}
        policy = {'schema_version': 1, 'exceptions': [exception]}
        today = dt.date(2026, 9, 20)
        self.assertEqual(policy_findings([finding()], policy, 'web', today), [])
        self.assertEqual(len(policy_findings([finding()], policy, 'android', today)), 1)
        self.assertEqual(len(policy_findings([{**finding(), 'modified': 'new'}], policy, 'web', today)), 1)
        with self.assertRaises(ScanError):
            policy_findings([], policy, 'web', dt.date(2026, 10, 1))
        exception['purl'] = '*'
        with self.assertRaises(ScanError):
            policy_findings([finding()], policy, 'web', today)

    def test_build_and_test_high_risk_also_requires_review(self):
        for scope in ('production', 'platform', 'build', 'test'):
            findings = [{**finding(), 'scopes': [scope]}]
            self.assertTrue(policy_findings(findings, {'schema_version': 1, 'exceptions': []}, 'all'))


class InventoryTests(unittest.TestCase):
    def test_current_inventory_and_sbom_are_deterministic(self):
        entries = inventory(ROOT)
        self.assertEqual(entries, inventory(ROOT))
        self.assertEqual(sbom(entries, 'windows', 'test'), sbom(entries, 'windows', 'test'))
        self.assertEqual(len(entries), len({(e['purl'], e.get('platform')) for e in entries}))
        self.assertEqual({e['ecosystem'] for e in entries}, {'crates.io', 'npm', 'Maven', 'native'})

    def test_pnpm_transitive_scopes_not_importer_dev_flag(self):
        entries = pnpm(ROOT)
        self.assertIn('production', next(e for e in entries if e['name'] == 'react')['scopes'])
        self.assertEqual(next(e for e in entries if e['name'] == 'vite')['scopes'], ['build'])
        self.assertIn('test', next(e for e in entries if e['name'] == 'playwright')['scopes'])

    def test_maven_multiple_scopes_and_editor_not_production(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('app', 'editor'):
                path = root / f'apps/android/{name}/gradle.lockfile'
                path.parent.mkdir(parents=True)
                path.write_text('group:artifact:1=releaseRuntimeClasspath,debugUnitTestRuntimeClasspath\n')
            entries = maven(root)
            self.assertEqual(entries[0]['scopes'], ['production', 'test'])
            self.assertEqual(entries[1]['scopes'], ['test'])


class SecretTests(unittest.TestCase):
    def test_redaction_and_exact_historical_allowance(self):
        canary = 'synthetic-sensitive-value'
        row = {'RuleID': 'test', 'File': 'fixture.txt', 'StartLine': 1, 'Fingerprint': 'known',
               'Secret': canary, 'Match': canary, 'Line': canary, 'Description': canary, 'Author': canary}
        clean = sanitized_secrets([row], {'known'}, 'history')
        self.assertNotIn(canary, json.dumps(clean))
        self.assertEqual(clean[0]['disposition'], 'known-synthetic')
        self.assertEqual(sanitized_secrets([row], {'known'}, 'artifact')[0]['disposition'], 'untriaged')
        self.assertEqual(sanitized_secrets([{**row, 'Fingerprint': 'different'}], {'known'}, 'history')[0]['disposition'], 'untriaged')

    def test_real_scanner_rejects_generated_secret_without_disclosing_it(self):
        # Deliberately fake; generated at runtime so this test does not add a
        # secret-shaped literal to repository history.
        canary = 'ghp_' + 'SYNTHETIC0123456789' * 2
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)
            git('init', '-q')
            git('config', 'user.email', 'fixture@example.invalid')
            git('config', 'user.name', 'Fixture')
            (root / 'canary.txt').write_text('token=' + canary + '\n')
            git('add', 'canary.txt')
            git('commit', '-qm', 'synthetic fixture')
            receipt = secrets(root, {'secret_fixtures': []})
            self.assertTrue(receipt['findings'])
            self.assertNotIn(canary, json.dumps(receipt))
            self.assertTrue(all(f['disposition'] == 'untriaged' for f in receipt['findings']))
            git('rm', 'canary.txt')
            git('commit', '-qm', 'remove synthetic fixture')
            self.assertTrue(secrets(root, {'secret_fixtures': []})['findings'])

    def test_binary_strings_and_artifact_secrets_are_scanned(self):
        canary = 'ghp_' + 'SYNTHETIC0123456789' * 2
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                            'commit', '--allow-empty', '-qm', 'fixture'], cwd=root, check=True)
            payload = root / 'payload'
            payload.mkdir()
            (payload / 'native.dll').write_bytes(b'MZ\x00' + canary.encode('utf-16-le') + b'\x00')
            strings = root / 'strings.txt'
            binary_strings(payload / 'native.dll', strings)
            self.assertIn(canary, strings.read_text())
            archive = root / 'release.zip'
            with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as zipped:
                zipped.writestr('config.txt', 'token=' + canary)
            report = secrets(root, {'secret_fixtures': []}, payload, [archive])
            self.assertNotIn(canary, json.dumps(report))
            self.assertTrue(any(f['scope'] == 'payload-binary-strings' for f in report['findings']))
            self.assertTrue(any(f['scope'] == 'artifact' for f in report['findings']))

    def test_payload_allowance_must_be_exact_and_relative(self):
        good = {'fingerprint': 'notices/build.json:generic-api-key:2', 'sha256': 'a' * 64, 'reason': 'hash'}
        self.assertEqual(payload_allowances({'payload_secret_allowances': [good]}), {good['fingerprint']: 'a' * 64})
        for bad in ({**good, 'fingerprint': '/tmp/stage/payload/notices/build.json:generic-api-key:2'},
                    {**good, 'fingerprint': 'notices/../x.json:generic-api-key:2'},
                    {**good, 'fingerprint': 'notices/build.json:generic-api-key'},
                    {**good, 'sha256': 'A' * 64}, {**good, 'reason': ''}):
            with self.subTest(bad=bad), self.assertRaises(ScanError):
                payload_allowances({'payload_secret_allowances': [bad]})
        with self.assertRaises(ScanError):
            payload_allowances({'payload_secret_allowances': [good, good]})

    def test_payload_match_is_allowed_only_for_its_exact_file_bytes(self):
        # A build-provenance style match, generated at runtime (no literal in history).
        value = hashlib.sha256(os.urandom(32)).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                            'commit', '--allow-empty', '-qm', 'fixture'], cwd=root, check=True)
            payload = root / 'stage' / 'payload'
            (payload / 'notices').mkdir(parents=True)
            notice = payload / 'notices' / 'build.txt'
            notice.write_text('source_api_key = "' + value + '"\n')
            report = secrets(root, {'secret_fixtures': []}, payload)
            matches = [f for f in report['findings'] if f['scope'] == 'payload']
            self.assertTrue(matches)
            self.assertTrue(all(f['file'] == 'notices/build.txt' and str(root) not in f['fingerprint'] for f in matches))
            allow = [{'fingerprint': f['fingerprint'], 'sha256': hashlib.sha256(notice.read_bytes()).hexdigest(),
                      'reason': 'fixture'} for f in report['findings'] if f['scope'].startswith('payload')]
            reviewed = secrets(root, {'secret_fixtures': [], 'payload_secret_allowances': allow}, payload)
            self.assertTrue(all(f['disposition'] == 'reviewed-payload' for f in reviewed['findings']
                                if f['scope'].startswith('payload')))
            notice.write_text('source_api_key = "' + value + '"\n# changed\n')
            changed = secrets(root, {'secret_fixtures': [], 'payload_secret_allowances': allow}, payload)
            self.assertTrue(all(f['disposition'] == 'untriaged' for f in changed['findings']
                                if f['scope'].startswith('payload')))

    def test_missing_scanner_fails(self):
        with patch('scanning.subprocess.run', side_effect=FileNotFoundError), self.assertRaises(ScanError):
            secrets(ROOT, {})


class PublicMetadataTests(unittest.TestCase):
    @staticmethod
    def signing_hash_fixture():
        # Synthetic hash metadata, not a CMS publisher signature or native app.
        code = b'synthetic-linked-code\0'
        offset = 48 + len(code)
        identifier = b'fixture\0'
        hash_offset = 44 + len(identifier)
        directory_size = hash_offset + 32
        placeholder = plistlib.dumps({'cdhashes': [b'x' * 20]})
        total = 28 + directory_size + 8 + len(placeholder)
        prefix = struct.pack('<IiiIIIII', 0xfeedfacf, 0x100000c, 0, 2, 1, 16, 0, 0)
        prefix += struct.pack('<IIII', 0x1d, 16, offset, total) + code
        directory = struct.pack('>9I4BI', 0xfade0c02, directory_size, 0x20001,
                                0, hash_offset, 44, 0, 1, offset, 32, 2, 0, 12, 0)
        directory += identifier + hashlib.sha256(prefix).digest()
        public = hashlib.sha256(directory).digest()[:20]
        xml = plistlib.dumps({'cdhashes': [public]})
        assert len(xml) == len(placeholder)
        wrapper = struct.pack('>II', 0xfade0b01, len(xml) + 8) + xml
        signature = struct.pack('>7I', 0xfade0cc0, total, 2,
                                0, 28, 0x10000, 28 + directory_size)
        return prefix + signature + directory + wrapper, base64.b64encode(public)

    def fixture(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        subprocess.run(['git', 'init', '-q', str(root)], check=True)
        subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                        'commit', '--allow-empty', '-qm', 'fixture'], cwd=root, check=True)
        payload = root / 'payload'
        payload.mkdir()
        return root, payload

    def test_verified_hash_does_not_suppress_same_value_outside_property(self):
        root, payload = self.fixture()
        blob, value = self.signing_hash_fixture()
        self.assertTrue(signature_regions(blob))
        # The identical public value is deliberately used as a credential-like
        # assignment outside its metadata span. Equality alone must not clear it.
        (payload / 'native.dat').write_bytes(blob + b'\0api_key="' + value + b'"\0')
        result = secrets(root, {'secret_fixtures': []}, payload)
        binary = [f for f in result['findings'] if f['scope'] == 'payload-binary-strings']
        self.assertTrue(any(f['disposition'] == 'verified-public-signing-hash' for f in binary))
        self.assertTrue(any(f['disposition'] == 'untriaged' for f in binary))
        self.assertNotIn(value.decode(), json.dumps(result))

    def test_changed_code_page_keeps_signing_hash_finding_untriaged(self):
        root, payload = self.fixture()
        blob, value = self.signing_hash_fixture()
        changed = bytearray(blob)
        changed[48] ^= 1
        self.assertEqual(signature_regions(bytes(changed)), [])
        (payload / 'native.dat').write_bytes(changed)
        result = secrets(root, {'secret_fixtures': []}, payload)
        self.assertTrue(result['findings'])
        self.assertTrue(all(f['disposition'] == 'untriaged' for f in result['findings']))
        self.assertNotIn(value.decode(), json.dumps(result))

    def test_resource_seal_checks_files_and_rejects_extra_credential_property(self):
        root, payload = self.fixture()
        contents = payload / 'Contents'
        (contents / 'Resources').mkdir(parents=True)
        (contents / '_CodeSignature').mkdir()
        resource = contents / 'Resources/data.txt'
        resource.write_bytes(b'synthetic resource')
        data = {'files': {'Resources/data.txt': hashlib.sha1(resource.read_bytes()).digest()},
                'files2': {'Resources/data.txt': {'hash2': hashlib.sha256(resource.read_bytes()).digest()}}}
        seal = contents / '_CodeSignature/CodeResources'
        seal.write_bytes(plistlib.dumps(data))
        self.assertEqual(len(resource_regions(seal.read_bytes(), payload, seal)), 2)
        result = secrets(root, {'secret_fixtures': []}, payload)
        self.assertTrue(any(f['disposition'] == 'verified-public-signing-hash' for f in result['findings']))
        resource.write_bytes(b'changed resource')
        self.assertEqual(resource_regions(seal.read_bytes(), payload, seal), [])
        changed = secrets(root, {'secret_fixtures': []}, payload)
        self.assertTrue(all(f['disposition'] == 'untriaged' for f in changed['findings']))
        data['api_key'] = hashlib.sha256(os.urandom(32)).hexdigest()
        seal.write_bytes(plistlib.dumps(data))
        self.assertEqual(resource_regions(seal.read_bytes(), payload, seal), [])
        malformed = secrets(root, {'secret_fixtures': []}, payload)
        self.assertTrue(malformed['findings'])
        self.assertTrue(all(f['disposition'] == 'untriaged' for f in malformed['findings']))
        self.assertNotIn(data['api_key'], json.dumps(malformed))

    def test_archive_copy_requires_exact_reviewed_payload_bytes(self):
        root, payload = self.fixture()
        (payload / 'notices').mkdir()
        notice = payload / 'notices/build.txt'
        value = hashlib.sha256(os.urandom(32)).hexdigest()
        notice.write_text('source_api_key="' + value + '"\n')
        baseline = secrets(root, {'secret_fixtures': []}, payload)
        policy = {'secret_fixtures': [], 'payload_secret_allowances': [
            {'fingerprint': f['fingerprint'], 'sha256': hashlib.sha256(notice.read_bytes()).hexdigest(),
             'reason': 'Synthetic exact-value test'} for f in baseline['findings']
            if f['scope'].startswith('payload')]}
        archive = root / 'release.zip'
        with zipfile.ZipFile(archive, 'w') as writer:
            writer.writestr('payload/notices/build.txt', notice.read_bytes())
        copied = secrets(root, policy, payload, [archive])
        artifact = [f for f in copied['findings'] if f['scope'] == 'artifact']
        self.assertTrue(artifact)
        self.assertTrue(all(f['disposition'] == 'reviewed-payload-copy' for f in artifact))
        with zipfile.ZipFile(archive, 'w') as writer:
            writer.writestr('payload/notices/build.txt', notice.read_bytes() + b'# changed\n')
        changed = secrets(root, policy, payload, [archive])
        artifact = [f for f in changed['findings'] if f['scope'] == 'artifact']
        self.assertTrue(artifact)
        self.assertTrue(all(f['disposition'] == 'untriaged' for f in artifact))
        self.assertNotIn(value, json.dumps(changed))

    def test_nested_tar_copy_and_duplicate_member_ambiguity(self):
        root, payload = self.fixture()
        (payload / 'notices').mkdir()
        inner = payload / 'notices/source.zip'
        value = hashlib.sha256(os.urandom(32)).hexdigest()
        with zipfile.ZipFile(inner, 'w') as writer:
            writer.writestr('source.txt', 'source_api_key="' + value + '"\n')
        baseline = secrets(root, {'secret_fixtures': []}, payload)
        policy = {'secret_fixtures': [], 'payload_secret_allowances': [
            {'fingerprint': f['fingerprint'], 'sha256': hashlib.sha256(inner.read_bytes()).hexdigest(),
             'reason': 'Synthetic nested public-copy test'} for f in baseline['findings']
            if f['scope'].startswith('payload') and '!' in f['file']]}
        archive = root / 'release.tar.gz'
        with tarfile.open(archive, 'w:gz') as writer:
            writer.add(inner, arcname='payload/notices/source.zip')
        copied = secrets(root, policy, payload, [archive])
        artifact = [f for f in copied['findings'] if f['scope'] == 'artifact']
        self.assertTrue(artifact)
        self.assertTrue(all(f['disposition'] == 'reviewed-payload-copy' for f in artifact))
        changed_inner = root / 'changed-source.zip'
        with zipfile.ZipFile(changed_inner, 'w') as writer:
            writer.writestr('source.txt', 'source_api_key="' + hashlib.sha256(os.urandom(32)).hexdigest() + '"\n')
        # One changed and one valid member have the same name and scanner
        # location. Choosing the last member would conceal the first finding.
        with tarfile.open(archive, 'w:gz') as writer:
            writer.add(changed_inner, arcname='payload/notices/source.zip')
            writer.add(inner, arcname='payload/notices/source.zip')
        changed = secrets(root, policy, payload, [archive])
        artifact = [f for f in changed['findings'] if f['scope'] == 'artifact']
        self.assertTrue(artifact)
        self.assertTrue(all(f['disposition'] == 'untriaged' for f in artifact))
        self.assertNotIn(value, json.dumps(changed))


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = self.root / 'payload'
        self.payload.mkdir()
        (self.payload / 'app.exe').write_bytes(b'MZsynthetic binary')
        (self.root / 'release.zip').write_bytes(b'synthetic container')
        self.commit = 'a' * 40
        self.data = {'schema_version': 1, 'platform': 'windows', 'source_commit': self.commit,
                     'builder': 'fixture', 'extraction_method': 'fixture', 'payload': 'payload',
                     'artifacts': [{'path': 'release.zip', 'sha256': digest(self.root / 'release.zip')}],
                     'files': {'app.exe': {'sha256': digest(self.payload / 'app.exe'), 'bytes': 18}},
                     'components': [{'name': 'synthetic', 'version': '1', 'files': ['app.exe'],
                                     'source': {'url': 'https://example.invalid/synthetic', 'revision': self.commit},
                                     'osv_query': {'commit': self.commit}}]}
        self.manifest = self.root / 'manifest.json'

    def run_manifest(self):
        self.manifest.write_text(json.dumps(self.data))
        return release_input(self.manifest, self.commit)

    def test_clean_fixture_binds_payload_source_and_container(self):
        self.assertEqual(len(self.run_manifest()[3]), 1)

    def test_tampered_extra_missing_and_unattributed_binaries_fail(self):
        self.run_manifest()
        (self.payload / 'extra.dll').write_bytes(b'MZextra')
        with self.assertRaises(ScanError):
            self.run_manifest()
        (self.payload / 'extra.dll').unlink()
        self.data['components'] = []
        with self.assertRaises(ScanError):
            self.run_manifest()
        (self.root / 'release.zip').write_bytes(b'changed')
        with self.assertRaises(ScanError):
            self.run_manifest()

    def test_vendor_binary_is_inventoried_without_advisory_identity(self):
        self.data['components'] = [{'name': 'microsoft/vc-runtime', 'version': '14.44.35211.0', 'files': ['app.exe'],
                                    'vendor': {'publisher': 'Microsoft Corporation',
                                               'authenticode_subject': 'CN=Microsoft Corporation'}}]
        entry = self.run_manifest()[3][0]
        self.assertNotIn('osv_query', entry)
        self.assertEqual(entry['evidence'], ['release-manifest-vendor'])
        for bad in ({'publisher': 'Microsoft Corporation'},
                    {'publisher': ' ', 'authenticode_subject': 'CN=Microsoft Corporation'}):
            self.data['components'][0]['vendor'] = bad
            with self.assertRaises(ScanError):
                self.run_manifest()
        self.data['components'][0]['vendor'] = {'publisher': 'Microsoft Corporation', 'authenticode_subject': 'CN=x'}
        self.data['components'][0]['osv_query'] = {'commit': self.commit}
        with self.assertRaises(ScanError):
            self.run_manifest()

    def test_wrong_commit_symlink_and_path_escape_fail(self):
        self.data['source_commit'] = 'b' * 40
        with self.assertRaises(ScanError):
            self.run_manifest()
        self.data['source_commit'] = self.commit
        (self.payload / 'escape').symlink_to('/etc/passwd')
        with self.assertRaises(ScanError):
            self.run_manifest()
        (self.payload / 'escape').unlink()
        self.data['artifacts'][0]['path'] = '../outside'
        with self.assertRaises(ScanError):
            self.run_manifest()


class ReceiptTests(unittest.TestCase):
    def test_complete_release_clean_fixture_then_tampering_expiry_and_risks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            policy = root / 'scripts/security/policy.json'
            policy.parent.mkdir(parents=True)
            policy.write_text(json.dumps({'schema_version': 1, 'exceptions': [], 'secret_fixtures': []}))
            (root / '.gitignore').write_text('.local/\n')
            subprocess.run(['git', 'add', '.'], cwd=root, check=True)
            subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                            'commit', '-qm', 'fixture'], cwd=root, check=True)
            commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root).decode().strip()
            stage = root / '.local'
            payload = stage / 'payload'
            payload.mkdir(parents=True)
            (payload / 'index.html').write_text('clean synthetic payload')
            archive = stage / 'release.zip'
            with zipfile.ZipFile(archive, 'w') as zipped:
                zipped.write(payload / 'index.html', 'index.html')
            manifest = stage / 'manifest.json'
            manifest.write_text(json.dumps({'schema_version': 1, 'platform': 'web', 'source_commit': commit,
                'builder': 'fixture', 'extraction_method': 'fixture', 'payload': 'payload',
                'files': {'index.html': {'bytes': (payload / 'index.html').stat().st_size,
                                         'sha256': digest(payload / 'index.html')}},
                'artifacts': [{'path': 'release.zip', 'sha256': digest(archive)}], 'components': []}))
            args = argparse.Namespace(command='release', manifest=str(manifest), ecosystem='all', platform='all',
                out=str(stage / 'receipt.json'), sbom=str(stage / 'bom.json'), receipt=str(stage / 'receipt.json'))
            def clean(entries):
                return osv(entries, lambda path, data=None: {'results': [{} for _ in data['queries']]})
            identity = stage / 'identity.json'
            identity.write_text(json.dumps({'schema_version': 1, 'terms': ['fixtureowner']}))
            with patch.object(gate, 'ROOT', root), patch.object(gate, 'inventory', return_value=[sample()]), \
                    patch.object(gate, 'osv', side_effect=clean), contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()), \
                    patch.dict(os.environ, {'WRITER_RELEASE_IDENTITY': str(identity)}):
                self.assertEqual(gate.run(args), 0)
                receipt = json.loads(Path(args.out).read_text())
                self.assertEqual(receipt['status'], 'passed')
                self.assertTrue(any(c['bom-ref'] == 'payload:index.html' for c in receipt['sbom']['components']))
                with patch.object(gate, 'osv', return_value=([finding()], {'queried_at': '2026-09-20T00:00:00Z'})):
                    self.assertEqual(gate.run(args), 1)
                    self.assertEqual(json.loads(Path(args.out).read_text())['status'], 'blocked')
                self.assertEqual(gate.run(args), 0)
                args.command = 'verify'
                self.assertEqual(gate.run(args), 0)
                receipt['advisory_database']['queried_at'] = '2000-01-01T00:00:00+00:00'
                Path(args.out).write_text(json.dumps(receipt))
                self.assertEqual(gate.run(args), 2)
                args.command = 'release'
                self.assertEqual(gate.run(args), 0)
                self.assertEqual(json.loads(Path(args.out).read_text())['personal_paths']['total'], 0)
                # A build-machine home path or owner name blocks the release (MW-RELEASE-14).
                for leak in ['panic at /home/fixture-builder/.cargo/registry/x.rs', 'built by FixtureOwner']:
                    (payload / 'index.html').write_text(leak)
                    data = json.loads(manifest.read_text())
                    data['files']['index.html'] = {'bytes': (payload / 'index.html').stat().st_size,
                                                   'sha256': digest(payload / 'index.html')}
                    manifest.write_text(json.dumps(data))
                    self.assertEqual(gate.run(args), 1)
                    receipt = json.loads(Path(args.out).read_text())
                    self.assertEqual([b['id'] for b in receipt['blocked']], ['PERSONAL-PATH'])
                    self.assertNotIn('fixture-builder', Path(args.out).read_text())
                    self.assertNotIn('FixtureOwner', Path(args.out).read_text())
                with patch.dict(os.environ, {'WRITER_RELEASE_IDENTITY': str(stage / 'missing.json')}):
                    self.assertEqual(gate.run(args), 2)
                (payload / 'index.html').write_text('tampered')
                args.command = 'verify'
                self.assertEqual(gate.run(args), 2)
                args.command = 'release'
                self.assertEqual(gate.run(args), 2)
                self.assertEqual(json.loads(Path(args.out).read_text())['status'], 'incomplete')

    def test_scanner_failure_replaces_previous_success(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / 'receipt.json'
            out.write_text('{"status":"passed"}')
            args = argparse.Namespace(command='scan', out=str(out))
            with patch.object(gate, 'source_receipt', side_effect=ScanError('required command failed')), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(gate.run(args), 2)
            self.assertEqual(json.loads(out.read_text())['status'], 'incomplete')


if __name__ == '__main__':
    unittest.main()
