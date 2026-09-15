import csv
import math
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from final_model import Scalar
import final_replay
from model import Config, Layout, Learner


class FinalContracts(unittest.TestCase):
    def test_scalar_matches_general_learner_over_sparse_and_outlier_rides(self):
        cfg = Config()
        scalar, general = Scalar(cfg), Learner(Layout(cfg, gradient=False))
        rng = np.random.default_rng(14)
        for ride in range(250):
            general.reset_ride()
            before = scalar.theta
            for _ in range(25):
                distance = float(rng.uniform(.0001, 1.)) * (.001 if ride % 3 == 0 else 1.)
                pace = math.exp(float(rng.normal(1., 2.)))
                initial = float(rng.normal(.8, .3))
                scalar.observe(initial, pace, distance)
                general.observe(np.ones(1, dtype=np.float32), initial, pace, distance)
            s = min(float(scalar.state[4])/cfg.ride_cap_km, 1.)
            self.assertTrue(scalar.finish())
            self.assertTrue(general.finish())
            np.testing.assert_allclose(scalar.state[:3], [general.theta[0], general.A[0], general.g[0]], rtol=3e-6, atol=2e-6)
            self.assertLessEqual(abs(scalar.theta-before), s*cfg.delta_global+1e-7)
            self.assertLessEqual(abs(scalar.theta), cfg.absolute_global+1e-7)
        self.assertEqual(scalar.state.nbytes, 20)

    def test_empty_finish_is_idempotent_and_invalid_state_is_atomic(self):
        scalar = Scalar(Config())
        self.assertFalse(scalar.finish())
        scalar.observe(0., 2., 10.)
        self.assertTrue(scalar.finish())
        before = scalar.state.copy()
        self.assertFalse(scalar.finish())
        self.assertFalse(scalar.observe(0., math.nan, 1.))
        np.testing.assert_equal(before, scalar.state)
        scalar.state[3:] = math.nan, 1.
        before = scalar.state.copy()
        self.assertFalse(scalar.finish())
        np.testing.assert_equal(before, scalar.state)

    def test_exact_replay_does_not_use_future_times_or_train_before_ride_end(self):
        cfg = Config(initial_pace=(3.,)*9, target_km=(1., 3.))
        values = np.array([[.1]*100, [.3]*100, [0.]*100, [0.]*100], dtype=np.float32)
        def replay(v, root):
            data = [(1, 1, 0, "bike", v)]
            with patch.object(final_replay, "OUTPUT", root), patch.object(final_replay, "read_rides", return_value=iter(data)):
                final_replay.run("calibration", cfg, {"log_offsets": {"other": -.1, "MTB": .3}})
            with (root / "calibration.csv").open() as f:
                return list(csv.DictReader(f))
        with tempfile.TemporaryDirectory() as temporary:
            before = replay(values, Path(temporary))
            altered = values.copy()
            altered[1, 40:] = .45
            after = replay(altered, Path(temporary))
        self.assertEqual([r["predicted_minutes"] for r in before], [r["predicted_minutes"] for r in after])
        self.assertNotEqual([r["actual_minutes"] for r in before], [r["actual_minutes"] for r in after])
        self.assertTrue(all(float(r["history_km"]) == 0 for r in before))
        self.assertEqual(len({(r["phase"], r["target_km"]) for r in before}), len(before)//2)


if __name__ == "__main__":
    unittest.main()
