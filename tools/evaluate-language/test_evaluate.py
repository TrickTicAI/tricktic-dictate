import copy
import json
from pathlib import Path
import unittest
from evaluate import evaluate, score

ROOT = Path(__file__).resolve().parents[2]

class EvaluationTests(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'tests/fixtures/language/samples.json').read_text())
        self.run = json.loads((ROOT / 'tests/fixtures/language/synthetic-run.json').read_text())

    def test_formatting_is_separate_from_omissions_and_correction_distance(self):
        result = score('Hello, world!', 'hello world')
        self.assertTrue(result['formatting_only'])
        self.assertEqual(result['word_edits'], 0)
        self.assertGreater(result['exact_character_edits_including_formatting'], 0)
        result = score('אחד שני שלוש', 'אחד שלוש')
        self.assertEqual([x['expected'] for x in result['omitted_tokens']], ['שני'])
        self.assertEqual(result['word_edits'], 1)

    def test_translation_requires_human_review_and_cleanup_does_not_replace_raw(self):
        original = copy.deepcopy(self.run)
        report = evaluate(self.manifest, self.run)
        self.assertEqual(report['missing_samples'], [])
        self.assertEqual(report['results'][4]['mixed_translation_gate'], 'fail')
        self.assertIn('hebrew', report['results'][4]['raw']['missing_script_flags'])
        self.assertTrue(report['results'][7]['cleanup']['exact_values_preserved'])
        self.assertEqual(self.run, original)
        del self.run['results'][4]['review']
        self.assertEqual(evaluate(self.manifest, self.run)['results'][4]['mixed_translation_gate'], 'unreviewed')

    def test_latency_is_separate_and_missing_is_not_zero(self):
        self.assertIsNone(evaluate(self.manifest, self.run)['results'][0]['stop_to_result_ms'])
        self.run['results'][0].update(stopped_at_ms=1000, result_at_ms=1250, recording_duration_ms=900)
        result = evaluate(self.manifest, self.run)['results'][0]
        self.assertEqual(result['stop_to_result_ms'], 250)
        self.assertEqual(result['recording_duration_ms'], 900)

    def test_real_audio_requires_consent_hash_and_complete_reference(self):
        self.run['kind'] = 'audio_run'
        with self.assertRaisesRegex(ValueError, 'consent'):
            evaluate(self.manifest, self.run)
        for result in self.run['results']:
            result.update(consent_confirmed=True, audio_sha256='0' * 64)
        with self.assertRaisesRegex(ValueError, 'placeholder'):
            evaluate(self.manifest, self.run)

    def test_invalid_language_duplicate_and_negative_latency_are_rejected(self):
        self.run['results'][0]['input_language'] = 'he'
        with self.assertRaisesRegex(ValueError, 'input_language'):
            evaluate(self.manifest, self.run)
        self.run['results'][0]['input_language'] = 'en'
        self.run['results'].append(self.run['results'][0])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            evaluate(self.manifest, self.run)
        self.run['results'].pop()
        self.run['results'][0].update(stopped_at_ms=1000, result_at_ms=900)
        with self.assertRaisesRegex(ValueError, 'timestamp'):
            evaluate(self.manifest, self.run)

if __name__ == '__main__':
    unittest.main()
