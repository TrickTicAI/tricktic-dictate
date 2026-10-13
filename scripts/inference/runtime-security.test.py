#!/usr/bin/env python3
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name+'.py'))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


package = load('package-runtime-upgrade')
ffmpeg = load('build-ffmpeg')


class RuntimeSecurity(unittest.TestCase):
    def test_source_archive_rejects_changed_bytes_or_size(self):
        with tempfile.TemporaryDirectory() as temp:
            file = Path(temp)/'archive'; file.write_bytes(b'original')
            pin = {'bytes': 8, 'sha256': hashlib.sha256(b'original').hexdigest()}
            ffmpeg.verify_archive(file, pin)
            for data in [b'modified', b'original plus bytes']:
                file.write_bytes(data)
                with self.assertRaises(ValueError): ffmpeg.verify_archive(file, pin)

    def test_package_rejects_tampering_additions_and_traversal(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); (root/'runtime').mkdir(); (root/'notices').mkdir()
            file = root/'runtime/tool'; file.write_bytes(b'original')
            entry = {'path': 'runtime/tool', 'bytes': 8, 'sha256': package.digest(file)}
            package.verify(root, [entry])
            with self.assertRaises(ValueError): package.verify(root, [entry, entry])
            with self.assertRaises(ValueError): package.verify(root, [{**entry, 'path': '../tool'}])
            (root/'runtime/injected').write_bytes(b'extra')
            with self.assertRaises(ValueError): package.verify(root, [entry])
            (root/'runtime/injected').unlink()
            file.write_bytes(b'modified')
            with self.assertRaises(ValueError): package.verify(root, [entry])
            file.unlink(); file.symlink_to(root/'notices/tool')
            (root/'notices/tool').write_bytes(b'original')
            with self.assertRaises(ValueError): package.verify(root, [entry])

    def test_only_reviewed_audio_entry_points_are_requested(self):
        args = ffmpeg.flags()
        self.assertIn('--disable-everything', args)
        self.assertIn('--disable-autodetect', args)
        self.assertIn('--disable-network', args)
        self.assertIn('--enable-protocol=file,pipe', args)
        self.assertIn('--enable-demuxer=matroska,ogg,mov,wav,mp3', args)

    def test_whisper_receipt_requires_complete_matching_backend_and_source(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); source = root/'source'; build = root/'build'
            (source/'src').mkdir(parents=True); (build/'bin').mkdir(parents=True)
            (source/'src/whisper.cpp').write_bytes(b'patched source')
            (source/'examples/server').mkdir(parents=True)
            (source/'examples/server/server.cpp').write_bytes(b'bounded parser')
            source_receipt = ('src/whisper.cpp='+package.digest(source/'src/whisper.cpp')+'\n'
                              'patched_server_sha256='+package.digest(source/'examples/server/server.cpp')+'\n')
            (source/'writer-source.txt').write_text(source_receipt)
            binary = build/'bin/whisper-server.exe'; binary.write_bytes(b'worker')
            receipt = {'source': source_receipt, 'backend': 'cpu',
                       'files': {binary.name: package.digest(binary)}}
            (build/'writer-build.json').write_text(json.dumps(receipt))
            package.verify_whisper(build, source, 'cpu', 'windows')
            (source/'examples/server/server.cpp').write_bytes(b'unbounded parser')
            with self.assertRaises(ValueError): package.verify_whisper(build, source, 'cpu', 'windows')
            (source/'examples/server/server.cpp').write_bytes(b'bounded parser')
            with self.assertRaises(ValueError): package.verify_whisper(build, source, 'vulkan', 'windows')
            binary.unlink()
            with self.assertRaises(ValueError): package.verify_whisper(build, source, 'cpu', 'windows')
            binary.write_bytes(b'worker')
            (source/'src/whisper.cpp').write_bytes(b'different source')
            with self.assertRaises(ValueError): package.verify_whisper(build, source, 'cpu', 'windows')


if __name__ == '__main__': unittest.main()
