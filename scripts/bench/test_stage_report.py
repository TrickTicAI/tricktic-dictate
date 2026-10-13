import importlib.util
from pathlib import Path
import unittest
import uuid
spec = importlib.util.spec_from_file_location('stage_report', Path(__file__).with_name('stage-report.py'))
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)

def row(ms=None, outcome='succeeded', ordinal=1, attempt=None):
    return dict(recording_id=str(uuid.uuid4()), attempt_id=attempt or str(uuid.uuid4()), retry_of=None, provider_attempt=ordinal, platform='windows', provider='local', model_sha256='a'*64, mode='regular', warm='warm', measurement='synthetic', outcome=outcome, stages_ms={} if ms is None else {'asr':ms}, stage_outcomes={})

class ReportTests(unittest.TestCase):
    def test_missing_failed_retried_duplicate_and_percentiles(self):
        attempt=str(uuid.uuid4())
        rows=[row(9),row(0),row(None),row(100,'failed',1,attempt),row(20,'succeeded',2,attempt)]
        rows.append(dict(rows[-1],stages_ms={'asr':30}))
        report=m.report(rows); group=report['groups'][0]; stage=group['stages']['asr']
        self.assertEqual(report['samples'],5); self.assertEqual(report['superseded_rows'],1)
        self.assertEqual(group['failures'],1); self.assertEqual(group['retries'],1)
        self.assertEqual(stage,dict(measured=4,absent=1,failures=1,successful_samples=3,p50_ms=9,p95_ms=30))
        self.assertIsNone(group['stages']['upload']['p95_ms'])
    def test_synthetic_physical_cold_warm_never_pool(self):
        rows=[row(1),dict(row(10),measurement='physical_device'),dict(row(50),warm='cold')]
        self.assertEqual(len(m.report(rows)['groups']),3)
    def test_content_rejected_even_short_or_nested(self):
        for extra in ({'raw_text':'hi'},{'context':{}},{'audio':'AAA'},{'credentials':[]}):
            with self.assertRaises(ValueError): m.validate(dict(row(1),**extra))
        for stages in ({'raw_text':1},{'asr':False},{'asr':-1},{'asr':1.5}):
            with self.assertRaises(ValueError): m.validate(dict(row(1),stages_ms=stages))
    def test_failed_cleanup_not_successful_latency(self):
        report=m.report([dict(row(10),stages_ms={'cleanup':500},stage_outcomes={'cleanup':'failed'})])
        stage=report['groups'][0]['stages']['cleanup']
        self.assertEqual(stage['failures'],1); self.assertIsNone(stage['p50_ms'])

if __name__=='__main__': unittest.main()
