"""Measurements retain failures and verify the complete release file set."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zlib

from tools import planner_bench as bench


class BenchmarkTests(unittest.TestCase):
    def test_complete_file_accounting_and_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = b'planner' * 1000
            (root / 'data').write_bytes(data)
            manifest = {'profiles': ['touring'], 'files': {'data': {
                'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}}}
            (root / 'release.json').write_text(json.dumps(manifest))
            measured = bench.audit(root)
            self.assertEqual(measured['installed_bytes'], len(data) + (root / 'release.json').stat().st_size)
            compressor = zlib.compressobj(6, zlib.DEFLATED, 31)
            self.assertEqual(measured['files']['data']['gzip6_bytes'], len(compressor.compress(data) + compressor.flush()))
            self.assertLess(measured['gzip6_or_raw_bytes'], measured['installed_bytes'])
            (root / 'data').write_bytes(data[:-1] + b'x')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                bench.audit(root)

    def test_failures_remain_in_latency_summary(self):
        samples = [{'profile': 'touring', 'elapsed_ms': n} for n in range(1, 20)]
        samples.append({'profile': 'touring', 'elapsed_ms': 1000, 'error': 'timeout'})
        row, = bench.summary(samples)
        self.assertEqual((row['samples'], row['failures'], row['p95_ms'], row['worst_ms']), (20, 1, 19, 1000))


if __name__ == '__main__':
    unittest.main()
