#!/usr/bin/env python3
"""Host checks for privacy, integrity, audio framing and quality accounting."""
import json
import importlib.util
import struct
from pathlib import Path
import tempfile
import unittest
import wave
from unittest.mock import patch

import device


class HarnessTests(unittest.TestCase):
    def test_private_paths_resolve_symlinks(self):
        with self.assertRaises(ValueError):
            device.private_path(device.ROOT / 'weights.bin')
        with tempfile.TemporaryDirectory() as directory:
            alias = Path(directory) / 'source'
            alias.symlink_to(device.REPO, target_is_directory=True)
            with self.assertRaises(ValueError):
                device.private_path(alias / 'audio.wav')

    def test_model_hash_and_size_are_both_required(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'model'
            path.write_bytes(b'abc')
            asset = {'id': 'fixture', 'bytes': 3, 'sha256': device.digest(path)}
            device.verify(path, asset)
            path.write_bytes(b'abd')
            with self.assertRaises(ValueError):
                device.verify(path, asset)
            path.write_bytes(b'ab')
            with self.assertRaises(ValueError):
                device.verify(path, asset)

    def test_pcm_preserves_extremes_and_rejects_wrong_rate(self):
        with tempfile.TemporaryDirectory() as directory:
            wav = Path(directory) / 'input.wav'
            pcm = Path(directory) / 'input.pcm'
            frames = b'\x00\x80\xff\x7f\x00\x00'
            with wave.open(str(wav), 'wb') as out:
                out.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
                out.writeframes(frames)
            self.assertEqual(device.pcm_from_wav(wav, pcm), 3 / 16)
            self.assertEqual(pcm.read_bytes(), frames)
            with wave.open(str(wav), 'wb') as out:
                out.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
                out.writeframes(frames)
            with self.assertRaises(ValueError):
                device.pcm_from_wav(wav, Path(directory) / 'bad.pcm')

    def test_truncated_audio_is_not_silently_shortened(self):
        with tempfile.TemporaryDirectory() as directory:
            wav = Path(directory) / 'input.wav'
            with wave.open(str(wav), 'wb') as out:
                out.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
                out.writeframes(b'\x00\x00' * 10)
            wav.write_bytes(wav.read_bytes()[:-2])
            with self.assertRaises(ValueError):
                device.pcm_from_wav(wav, Path(directory) / 'bad.pcm')

    def test_offline_fails_closed_for_unknown_or_active_network(self):
        state = {'airplane': '1', 'wifi': '0', 'bluetooth': '0', 'global_addresses': ''}
        self.assertTrue(device.offline(state))
        for field, value in [('airplane', 'null'), ('wifi', '1'), ('bluetooth', '1'), ('global_addresses', 'wlan0 inet 192.0.2.1')]:
            self.assertFalse(device.offline({**state, field: value}))

    def test_wer_counts_omissions_insertions_and_language(self):
        self.assertEqual(device.wer('Hello, world!', 'hello world'), 0)
        self.assertEqual(device.wer('שלום Alex', 'שָׁלוֹם alex'), 0)
        self.assertEqual(device.wer('one two three', 'one three'), 1 / 3)
        self.assertEqual(device.wer('שלום עולם', 'hello world'), 1)
        self.assertEqual(device.wer('one', 'one two three'), 2)
        self.assertIsNone(device.wer('', 'hallucination'))

    def test_partial_output_cannot_be_summarized_as_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / 'suite.json').write_text(json.dumps({'clips': [{'id': 'en', 'mode': 'en', 'reference': 'one two'}]}))
            row = {'event': 'result', 'clip': 'en', 'raw_text': 'one', 'inference_ms': 50, 'audio_ms': 100}
            log = output / 'results.jsonl'
            log.write_text(json.dumps(row) + '\n')
            with self.assertRaises(ValueError):
                device.summarize(output)
            log.write_text(log.read_text() + '{"event":"complete"}\n')
            device.summarize(output)
            quality = json.loads((output / 'quality-review.json').read_text())[0]
            self.assertEqual(quality['wer'], 0.5)
            self.assertNotIn('raw_text', quality)
            self.assertIsNone(quality['meaning_preserved'])
            (output / 'suite.json').write_text(json.dumps({'clips': [{'id': 'en', 'mode': 'en', 'reference': 'one two', 'reference_kind': 'model_baseline'}]}))
            device.summarize(output)
            baseline = json.loads((output / 'quality-review.json').read_text())[0]
            self.assertIsNone(baseline['wer'])
            self.assertEqual(baseline['baseline_disagreement'], 0.5)
            (output / 'suite.json').write_text(json.dumps({'clips': [{'id': 'en', 'mode': 'en', 'reference': 'one two', 'reference_kind': 'prompt_unconfirmed'}]}))
            device.summarize(output)
            provisional = json.loads((output / 'quality-review.json').read_text())[0]
            self.assertIsNone(provisional['wer'])
            self.assertEqual(provisional['prompt_disagreement'], 0.5)

    def test_public_float_conversion_saturates_without_resampling(self):
        spec = importlib.util.spec_from_file_location('fleurs', Path(__file__).with_name('fetch-public-he.py'))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / 'float.wav', Path(directory) / 'pcm.wav'
            fmt = struct.pack('<HHIIHH', 3, 1, 16000, 64000, 4, 32)
            data = struct.pack('<ffff', -1.1, -0.5, 0.5, 1.1)
            chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(data)) + data
            source.write_bytes(b'RIFF' + struct.pack('<I', len(chunks) + 4) + b'WAVE' + chunks)
            self.assertEqual(module.convert_float_wav(source, target), 4)
            with wave.open(str(target), 'rb') as audio:
                self.assertEqual(audio.getframerate(), 16000)
                self.assertEqual(struct.unpack('<hhhh', audio.readframes(4)), (-32768, -16384, 16384, 32767))
            source.write_bytes(source.read_bytes()[:-1])
            with self.assertRaises(ValueError):
                module.convert_float_wav(source, target)

    def test_cancel_only_signals_the_recorded_owned_process(self):
        with patch.object(device, 'shell', side_effect=['123', '/unrelated/process']) as shell:
            device.stop_owned('explicit', '/data/local/tmp/writer-test')
            self.assertEqual(shell.call_count, 2)
        with patch.object(device, 'shell', side_effect=['123', '/data/local/tmp/writer-test/bench\0', '']) as shell:
            device.stop_owned('explicit', '/data/local/tmp/writer-test')
            shell.assert_called_with('explicit', 'kill', '-INT', '123')


if __name__ == '__main__':
    unittest.main()
