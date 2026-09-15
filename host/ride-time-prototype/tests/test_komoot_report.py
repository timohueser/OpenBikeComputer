"""Independent arithmetic and pairing checks for reported accuracy."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from komoot_report import metrics, paired_difference


class KomootReportTests(unittest.TestCase):
    def rows(self):
        return [dict(ride='a', at_minutes=10, actual_minutes=100, predicted_minutes=120,
                     low_minutes=110, high_minutes=130),
                dict(ride='b', at_minutes=10, actual_minutes=200, predicted_minutes=180,
                     low_minutes=190, high_minutes=210)]

    def test_ride_weighted_errors_and_coverage(self):
        m = metrics(self.rows())
        self.assertEqual(m['n'], 2)
        self.assertEqual(m['mape'], 15)
        self.assertEqual(m['mae_minutes'], 20)
        self.assertEqual(m['bias_minutes'], 0)
        self.assertEqual(m['coverage'], 50)
        self.assertEqual(m['median_width_minutes'], 20)
        self.assertAlmostEqual(m['p90_ape'], 19)

    def test_pairing_uses_identity_and_rejects_missing_or_changed_targets(self):
        a = self.rows()
        b = [dict(a[1], predicted_minutes=200), dict(a[0], predicted_minutes=110)]
        m = paired_difference(a, b)
        self.assertEqual(m['improved'], 2)
        self.assertEqual(m['mean_delta_pp'], -10)
        for wrong in (b[:1], [b[0], b[0]], [b[0], dict(b[1], actual_minutes=101)]):
            with self.assertRaises(ValueError):
                paired_difference(a, wrong)


if __name__ == '__main__':
    unittest.main()
