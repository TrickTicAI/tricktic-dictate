import json
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile
import contextlib
import io
from unittest.mock import patch

import personal_paths
from scanning import ScanError

TERMS = ['fixtureowner', 'fixture-host']
ALLOW = [{'text': b'Fixtureowner Signer', 'scope': 'code-signature'},
         {'text': b'Copyright Fixtureowner Thirdparty', 'scope': 'any'}]


def macho(signature):
    """Minimal 64-bit Mach-O whose LC_CODE_SIGNATURE covers `signature`."""
    header = struct.pack('<IIIIIIII', 0xfeedfacf, 0x0100000c, 0, 2, 1, 16, 0, 0)
    offset = len(header) + 16 + 64
    command = struct.pack('<IIII', 0x1d, 16, offset, len(signature))
    return header + command + b'\x00' * 64 + signature


class PersonalPathTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def scan(self, *paths):
        return personal_paths.scan(paths or [self.root / 'artifact'], TERMS, ALLOW)

    def write(self, relative, data):
        path = self.root / 'artifact' / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def test_clean_artifact_passes(self):
        self.write('app/main', b'\x7fELF' + b'/rustc/abc/library/core/src/panic.rs\x00~/.cache/x\x00/writer/src/ggml.c')
        self.write('notices/LICENSE', b'Copyright Fixtureowner Thirdparty and contributors\n/home/\n/Users/Shared/x')
        report = self.scan()
        self.assertEqual(report['status'], 'passed')
        self.assertEqual(report['files'], 2)

    def test_home_paths_in_binaries_are_found_in_ascii_and_utf16(self):
        self.write('app/main', b'\x7fELF\x00/home/builder7/.cargo/registry/src/x.rs\x00/Users/someone/src/a.c\x00')
        self.write('app/win.exe', b'MZ' + 'C:\\Users\\someone\\AppData\\x.pdb'.encode('utf-16le'))
        self.write('app/escaped.json', b'{"cc": "C:\\\\Users\\\\someone\\\\llvm"}')
        report = self.scan()
        counts = {(f['file'].split('/')[-1], f['category']): f['count'] for f in report['findings']}
        self.assertEqual(counts, {('main', 'home-path'): 2, ('win.exe', 'home-path'): 1, ('escaped.json', 'home-path'): 1})
        self.assertEqual(report['status'], 'blocked')

    def test_owner_terms_respect_word_boundaries_and_allow_list(self):
        self.write('a.txt', b'built on fixture-host by FixtureOwner\n')
        self.write('b.txt', b'fixtureownerless dashed fixture-hosting')
        self.write('c.txt', b'Copyright Fixtureowner Thirdparty')
        report = self.scan()
        self.assertEqual([(f['file'], f['count']) for f in report['findings']], [('artifact/a.txt', 2)])

    def test_signer_name_is_allowed_only_inside_a_code_signature(self):
        self.write('signed', macho(b'\xfa\xde\x0c\xc0 Developer ID Application: Fixtureowner Signer (TEAM)'))
        self.assertEqual(self.scan()['status'], 'passed')
        self.write('unsigned', b'\x7fELF built by Fixtureowner Signer')
        report = self.scan()
        self.assertEqual([f['file'] for f in report['findings']], ['artifact/unsigned'])

    def test_owner_terms_in_a_signature_need_no_allow_entry_but_home_paths_do(self):
        self.write('signed', macho(b'\xfa\xde\x0c\xc0 Developer ID Application: Fixtureowner Other (TEAM)'))
        self.assertEqual(personal_paths.scan([self.root / 'artifact'], TERMS, [])['status'], 'passed')
        self.write('signed', macho(b'\xfa\xde\x0c\xc0 /home/builder7/cert.pem'))
        self.assertEqual(personal_paths.scan([self.root / 'artifact'], TERMS, [])['status'], 'blocked')

    def test_disk_image_signature_is_recognized(self):
        body = b'koly-free image data ' * 4
        signature = b'Developer ID Application: Fixtureowner Signer (TEAM)'
        trailer = bytearray(512)
        trailer[:4] = b'koly'
        struct.pack_into('>QQ', trailer, 296, len(body), len(signature))
        self.write('image.bin', body + signature + bytes(trailer))
        self.assertEqual(self.scan()['status'], 'passed')
        struct.pack_into('>QQ', trailer, 296, 0, 0)
        self.write('image.bin', body + signature + bytes(trailer))
        self.assertEqual(self.scan()['status'], 'blocked')

    def test_nested_containers_are_unpacked(self):
        inner = io.BytesIO()
        with zipfile.ZipFile(inner, 'w', zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('lib/classes.dex', b'dex\n/home/someone/AndroidStudioProjects/a.kt')
        outer = self.root / 'release.tar.gz'
        with tarfile.open(outer, 'w:gz') as archive:
            info = tarfile.TarInfo('App.app/Contents/Resources/inner.apk')
            info.size = len(inner.getvalue())
            archive.addfile(info, io.BytesIO(inner.getvalue()))
        report = personal_paths.scan([outer], TERMS, ALLOW)
        self.assertEqual(report['containers'], 2)
        self.assertEqual([f['file'] for f in report['findings']],
                         ['release.tar.gz!/App.app/Contents/Resources/inner.apk!/lib/classes.dex'])
        raw = personal_paths.scan([outer], TERMS, ALLOW, containers=False)
        self.assertEqual((raw['containers'], raw['status']), (0, 'passed'))

    def test_nsis_without_seven_zip_fails_instead_of_passing(self):
        installer = self.write('setup.exe', b'MZ' + b'\x00' * 100 + b'\xef\xbe\xad\xdeNullsoftInst')
        with self.assertRaises(ScanError):
            personal_paths.scan([installer], TERMS, ALLOW, tool=None)

    def test_report_never_contains_the_matched_names(self):
        self.write('main', b'/home/builder7/src/x.c fixture-host /Users/someone/y C:\\Users\\someone\\z')
        out = self.root / 'report.json'
        with patch.dict(os.environ, {'WRITER_RELEASE_IDENTITY': str(self.identity())}), \
                contextlib.redirect_stdout(io.StringIO()):
            code = personal_paths.main([str(self.root / 'artifact'), '--out', str(out)])
        self.assertEqual(code, 1)
        text = out.read_text()
        for secret in ['builder7', 'someone', 'fixture-host']:
            self.assertNotIn(secret, text)
        self.assertIn('/home/<home>/src/x.c', text)

    def identity(self):
        path = self.root / 'identity.json'
        path.write_text(json.dumps({'schema_version': 1, 'terms': TERMS}))
        return path

    def test_identity_file_is_required_and_adds_this_host(self):
        with self.assertRaises(ScanError):
            personal_paths.identity_terms(self.root / 'missing.json', env={})
        terms = personal_paths.identity_terms(self.identity(), env={})
        self.assertTrue(set(TERMS) <= set(terms))
        self.assertIn(Path.home().name, terms + list(personal_paths.GENERIC_ACCOUNTS))
        with patch.object(personal_paths.getpass, 'getuser', return_value='root'):
            self.assertNotIn('root', personal_paths.identity_terms(self.identity(), env={}))

    def test_tracked_allow_list_entries_have_reasons(self):
        data = json.loads(personal_paths.ALLOW_FILE.read_text())
        self.assertTrue(data['allow'])
        self.assertTrue(all(item['reason'] for item in data['allow']))
        self.assertEqual(len(personal_paths.load_allow()), len(data['allow']))


if __name__ == '__main__':
    unittest.main()
