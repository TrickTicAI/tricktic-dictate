"""Build-input guard tests using synthetic Git fixtures, not native acceptance."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


HERE = Path(__file__).resolve().parent
stage = load('stage_macos_text', HERE / 'stage-macos-text.py')
build = load('build_macos_text', HERE / 'build-macos-text.py')
replace = load('replace_macos_text', HERE.parent / 'desktop/replace-macos-text.py')
sha = lambda data: hashlib.sha256(data).hexdigest()


class SourceGuards(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'upstream'
        self.source.mkdir()
        self.git('init', '-q')
        self.git('config', 'user.name', 'Synthetic test')
        self.git('config', 'user.email', 'test@example.invalid')
        (self.source / 'vendor/miniaudio').mkdir(parents=True)
        (self.source / 'vendor/miniaudio/miniaudio.h').write_bytes(b'old synthetic header')
        (self.source / 'CMakeLists.txt').write_bytes(b'synthetic build input')
        (self.source / '.gitignore').write_text('ignored-input.h\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'synthetic pin')
        self.header, self.license = self.root / 'header', self.root / 'license'
        self.header.write_bytes(b'fixed synthetic header')
        self.license.write_bytes(b'synthetic license')
        self.pin = {'llama': {'revision': self.git('rev-parse', 'HEAD').decode().strip()},
                    'miniaudio': {'sha256': sha(self.header.read_bytes()),
                                  'license_sha256': sha(self.license.read_bytes())}}
        self.output = self.root / 'staged'

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.source), *args], stderr=subprocess.DEVNULL)

    def prepare(self):
        return stage.stage(self.source, self.header, self.license, self.output, self.pin)

    def test_staging_preserves_dirty_input_and_uses_independent_git_objects(self):
        (self.source / 'CMakeLists.txt').write_bytes(b'unrelated uncommitted work')
        before = self.git('status', '--porcelain')
        self.prepare()
        self.assertEqual(self.git('status', '--porcelain'), before)
        self.assertEqual((self.source / 'vendor/miniaudio/miniaudio.h').read_bytes(), b'old synthetic header')
        self.assertEqual((self.output / 'CMakeLists.txt').read_bytes(), b'synthetic build input')
        self.assertFalse((self.output / '.git/objects/info/alternates').exists())
        self.assertEqual(build.validate_source(self.output, self.pin)['input_clone_modified'], False)
        upstream = list((self.source / '.git/objects').glob('*/*'))
        for f in upstream:
            other = self.output / '.git/objects' / f.relative_to(self.source / '.git/objects')
            if other.is_file():
                self.assertNotEqual(f.stat().st_ino, other.stat().st_ino)

    def test_wrong_header_license_or_pin_fails_before_creating_output(self):
        for field in ['sha256', 'license_sha256']:
            bad = copy.deepcopy(self.pin)
            bad['miniaudio'][field] = '0' * 64
            with self.assertRaises(ValueError):
                stage.stage(self.source, self.header, self.license, self.output, bad)
            self.assertFalse(self.output.exists())
        bad = copy.deepcopy(self.pin)
        bad['llama']['revision'] = '0' * 40
        with self.assertRaises(subprocess.CalledProcessError):
            stage.stage(self.source, self.header, self.license, self.output, bad)
        self.assertFalse(self.output.exists())

    def test_rejects_existing_output_and_input_overlap(self):
        for output in [self.source, self.source / 'nested', HERE / 'synthetic-output']:
            with self.assertRaises(ValueError):
                stage.stage(self.source, self.header, self.license, output, self.pin)

    def test_build_rejects_tampering_and_ignored_input_injection(self):
        self.prepare()
        for relative in ['vendor/miniaudio/miniaudio.h', 'writer-miniaudio-LICENSE.txt', 'CMakeLists.txt']:
            path = self.output / relative
            saved = path.read_bytes()
            path.write_bytes(saved + b' changed')
            with self.assertRaises(ValueError):
                build.validate_source(self.output, self.pin)
            path.write_bytes(saved)
        receipt = self.output / 'writer-source.json'
        original = receipt.read_bytes()
        altered = json.loads(original)
        altered['vendored_miniaudio_original_sha256'] = '0' * 64
        receipt.write_text(json.dumps(altered))
        with self.assertRaises(ValueError):
            build.validate_source(self.output, self.pin)
        receipt.write_bytes(original)
        (self.output / 'ignored-input.h').write_bytes(b'unapproved ignored build input')
        with self.assertRaises(ValueError):
            build.validate_source(self.output, self.pin)


class PackageGuards(unittest.TestCase):
    def test_retained_package_rejects_changed_extra_and_symlink_inputs(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            f = base / 'retained'
            f.write_bytes(b'synthetic retained bytes')
            lock = {'version': 1, 'target': 'aarch64-apple-darwin',
                    'files': [{'path': 'retained', 'bytes': f.stat().st_size, 'sha256': sha(f.read_bytes())}]}
            self.assertEqual(set(replace.verified_files(base, lock)), {'retained'})
            f.write_bytes(b'synthetic changed bytes!')
            with self.assertRaises(ValueError):
                replace.verified_files(base, lock)
            f.write_bytes(b'synthetic retained bytes')
            extra = base / 'unlisted'
            extra.write_bytes(b'additional input')
            with self.assertRaises(ValueError):
                replace.verified_files(base, lock)
            extra.unlink()
            extra.symlink_to(f)
            with self.assertRaises(ValueError):
                replace.verified_files(base, lock)

    def test_text_package_rejects_changed_build_outputs_and_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pin = {'llama': {'revision': 'synthetic-pin'},
                   'miniaudio': {'license_sha256': sha(b'synthetic license')}}
            for backend in ['cpu', 'metal']:
                directory = root / ('llama-' + backend)
                package = directory / 'package'
                package.mkdir(parents=True)
                (package / 'llama-server').write_bytes(b'synthetic runtime')
                (directory / 'miniaudio-LICENSE.txt').write_bytes(b'synthetic license')
                receipt = {'llama': pin['llama'], 'miniaudio': pin['miniaudio'],
                           'revision': 'synthetic-pin', 'backend': backend,
                           'files': {'llama-server': sha(b'synthetic runtime')},
                           'configure': ['-DGGML_RPC=OFF', '-DGGML_OPENMP=OFF',
                                         '-DCMAKE_OSX_ARCHITECTURES=arm64',
                                         '-DGGML_METAL=' + ('ON' if backend == 'metal' else 'OFF')]}
                (directory / 'writer-build.json').write_text(json.dumps(receipt))
            self.assertEqual(set(replace.build_inputs(root, pin)), {'cpu', 'metal'})
            file = root / 'llama-cpu/package/llama-server'
            file.write_bytes(b'changed runtime')
            with self.assertRaises(ValueError):
                replace.build_inputs(root, pin)
            file.write_bytes(b'synthetic runtime')
            receipt_path = root / 'llama-cpu/writer-build.json'
            receipt = json.loads(receipt_path.read_text())
            receipt['configure'].remove('-DGGML_RPC=OFF')
            receipt_path.write_text(json.dumps(receipt))
            with self.assertRaises(ValueError):
                replace.build_inputs(root, pin)


if __name__ == '__main__':
    unittest.main()
