"""Causality, identifiability and bounds for exploratory corrections."""

from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from endurance import Correction, LIMIT, centered_moments, duration_shape, predict, replay
from final_model import Scalar
from model import Config, Intervals
from long_replay import ride_replay


class EnduranceTests(unittest.TestCase):
    def test_grade_centering_removes_uphill_finish_confound(self):
        x = np.array([0., 0., 1., 1.])
        y = np.array([0., 0., .5, .5])
        groups = np.array([0, 0, 1, 1])
        self.assertEqual(centered_moments(x, y, np.ones(4), groups), (0., 0.))
        correction = Correction("duration")
        correction.finish(x, y, np.ones(4), groups)
        self.assertEqual(correction.beta, 0.)
        self.assertEqual(correction.updates, 0)

    def test_identified_effect_is_bounded_and_not_updated_without_evidence(self):
        x = np.array([0., 1., 0., 1.])
        correction = Correction("duration")
        for _ in range(100):
            before = correction.beta
            correction.finish(x, x*100, np.ones(4)*10, np.zeros(4))
            self.assertLessEqual(abs(correction.beta-before), .030000001)
            self.assertLessEqual(correction.beta, LIMIT)
        previous = (correction.beta, correction.A, correction.h)
        correction.finish(x*0, x, np.ones(4), np.zeros(4))
        self.assertEqual(previous, (correction.beta, correction.A, correction.h))

    def test_duration_forecast_counts_only_additional_future_slowing(self):
        beta = np.log(1.4)
        # Current fatigue has been removed from the live residual; at saturation
        # the forecast must apply it once, even over many future blocks.
        base = np.ones(100)
        result = predict(base, np.zeros(100), "duration", beta, 0., 400.)
        self.assertAlmostEqual(result, 140.)
        self.assertEqual(float(duration_shape(120)), 0.)
        self.assertEqual(float(duration_shape(360)), 1.)

    def test_future_times_do_not_enter_issued_duration_forecast(self):
        cfg = Config()
        values = np.array([np.ones(200)*.1, np.ones(200), np.zeros(200), np.ones(200)])
        zeros = np.zeros(200)
        correction = Correction("duration")
        correction.beta = .2
        first, _ = replay((values, zeros, zeros, zeros), cfg, Scalar(cfg), correction)
        changed = values.copy()
        changed[1, 40:] *= 3
        other = Correction("duration")
        other.beta = .2
        second, _ = replay((changed, zeros, zeros, zeros), cfg, Scalar(cfg), other)
        for a, b in zip(first[:4], second[:4]):
            self.assertEqual(a["predicted_minutes"], b["predicted_minutes"])
            self.assertEqual(a["coefficient"], .2)

    def test_baseline_matches_previous_replay(self):
        cfg = Config()
        values = np.array([np.ones(100)*.1, np.ones(100), np.zeros(100), np.ones(100)])
        zero = np.zeros(100)
        learner = Scalar(cfg)
        reference = [np.linspace(-.5, .5, 64)]*6
        expected = ride_replay(values, cfg, learner, Intervals(cfg, reference))
        actual, _ = replay((values, zero, zero, zero), cfg, Scalar(cfg), Correction("baseline"))
        np.testing.assert_allclose([r["predicted_minutes"] for r in actual],
                                   [r["predicted_minutes"] for r in expected], rtol=1e-12)


if __name__ == "__main__":
    unittest.main()
