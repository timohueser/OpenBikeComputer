from pathlib import Path
import tempfile
import unittest
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import test_cost


class TestCostTests(unittest.TestCase):
    def test_parallel_jobs_count_cost_separately_and_reject_wrong_attempts(self):
        run = {'id': 7, 'run_attempt': 2, 'run_started_at': '2026-01-01T00:00:00Z'}
        job = {'run_id': 7, 'run_attempt': 2, 'status': 'completed', 'conclusion': 'success',
               'started_at': '2026-01-01T00:01:00Z', 'completed_at': '2026-01-01T00:03:00Z'}
        data = {'total_count': 3, 'jobs': [job, dict(job), {**job, 'conclusion': 'skipped'}]}
        self.assertEqual(test_cost.run_cost(run, data), (3, 4))
        with self.assertRaisesRegex(ValueError, 'another run attempt'):
            test_cost.run_cost({**run, 'run_attempt': 3}, data)
        with self.assertRaisesRegex(ValueError, 'incomplete job pagination'):
            test_cost.run_cost(run, {**data, 'total_count': 4})

    def test_native_durations_keep_identity_and_missing_times_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = root / 'native.xml'
            report.write_text('<testsuites><testsuite name="binary" time="2.5"/>'
                              '<testsuite name="unknown"/><testsuite name="zero" time="0"/></testsuites>')
            self.assertEqual(test_cost.native_times(root).suites, {'native.xml::binary': 2.5, 'native.xml::zero': 0.0})
            report.write_text('<testsuites><testsuite name="bad" time="nan"/></testsuites>')
            with self.assertRaisesRegex(ValueError, 'invalid native suite duration'):
                test_cost.native_times(root)
