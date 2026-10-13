import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('pause_report', Path(__file__).with_name('pause-reuse-report.py'))
reporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reporter)


class PauseReportTests(unittest.TestCase):
    def data(self):
        fixture = {'samples': [{'id': 'speech', 'input_language': 'en', 'expected_text': 'hello', 'audio_sha256': 'a' * 64}]}
        base = [{'id': 'speech', 'repeat': i, 'baseline': True, 'warm': i > 0,
                 'source_sha256': 'a' * 64, 'derived_pcm_sha256': 'b' * 64,
                 'first_preview_ms': 500, 'extension_preview_ms': 600,
                 'stop_to_raw_ms': 600 + i, 'raw_text': 'hello',
                 'stages': {'details': {'preview_inference_calls': 2}}} for i in range(3)]
        candidate = copy.deepcopy(base)
        for row in candidate:
            row.update(baseline=False, extension_preview_ms=5, stop_to_raw_ms=10)
            row['stages']['details']['preview_inference_calls'] = 1
        return fixture, base, candidate

    def test_counts_percentiles_absent_language_and_content_exclusion(self):
        result = reporter.report(*self.data())
        warm = result['arms']['baseline']['cohorts']['warm']
        self.assertEqual((warm['n'], warm['stop_to_raw_ms']['p50_ms'], warm['stop_to_raw_ms']['p95_ms']), (2, 601, 602))
        self.assertEqual(warm['preview_inference_calls'], 4)
        self.assertEqual(result['arms']['candidate']['quality']['mixed']['wer'], None)
        self.assertEqual(result['changed_final_outputs'], 0)
        self.assertNotIn('hello', str(result))

    def test_rejects_unpaired_audio_duplicate_and_missing_trace(self):
        for mutate in [lambda rows: rows.pop(),
                       lambda rows: rows.append(copy.deepcopy(rows[0])),
                       lambda rows: rows[0].update(derived_pcm_sha256='c' * 64),
                       lambda rows: rows[0]['stages'].update(details={}),
                       lambda rows: rows[0].update(stop_to_raw_ms=float('nan'))]:
            fixture, baseline, candidate = self.data()
            mutate(candidate)
            with self.assertRaises(ValueError):
                reporter.report(fixture, baseline, candidate)

    def test_quality_regression_is_visible(self):
        fixture, baseline, candidate = self.data()
        candidate[0]['raw_text'] = 'goodbye'
        result = reporter.report(fixture, baseline, candidate)
        self.assertEqual(result['changed_final_outputs'], 1)
        self.assertGreater(result['arms']['candidate']['quality']['en']['wer'], 0)
