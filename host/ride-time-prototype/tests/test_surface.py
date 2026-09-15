from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from matching_v2 import PathNetwork
from model import Config, Layout
from surface_data import causal_compositions
from surface_model import MODELS, correction, features, fit
import surface_replay


class SurfaceContracts(unittest.TestCase):
    def test_causal_matching_is_prefix_invariant_and_resets_at_gaps(self):
        ways = [dict(id=1, nodes=[1, 2], coordinates=[[0, 0], [.002, 0]],
                     tags={"highway": "track", "surface": "gravel"}),
                dict(id=2, nodes=[3, 4], coordinates=[[0, .00015], [.002, .00015]],
                     tags={"highway": "cycleway", "surface": "asphalt"})]
        graph = PathNetwork(ways, [0, 0])
        track = np.array([[.0001, 0, 0], [.0004, 0, 10], [.0007, 0, 20],
                          [.001, .00015, 30], [.0013, .00015, 70], [.0016, .00015, 80]])
        full = causal_compositions(graph, track)
        self.assertTrue(full[0])
        self.assertEqual(full[3], {})
        for end in range(2, len(track)):
            self.assertEqual(causal_compositions(graph, track[:end]), full[:end-1])

    def test_partial_composition_keeps_unknown_neutral_and_total_correction_bounded(self):
        x = features([{}, {"highway": {"path": .6}, "surface": {"gravel": .3, "dirt": .2}}], "bike")
        np.testing.assert_equal(x[0], [1, 0, 0, 0, 0, 0, 0, 0, 0])
        np.testing.assert_equal(x[1], [1, 0, 0, .6, 0, .3, .2, 0, 0])
        self.assertLessEqual(float(np.max(correction(x, np.full(9, 1e9)))), np.log(3))

    def test_shared_fit_learns_within_ride_surface_difference(self):
        cfg = Config(initial_pace=(1.5,)*9)
        values = np.array([[.1]*40, [.15]*20+[.3]*20, [0.]*40, [0.]*40], dtype=np.float32)
        tags = [{"surface": {"asphalt": 1}}]*20+[{"surface": {"gravel": 1}}]*20
        rides = [(dict(user=u, sport="bike"), values, {"causal": tags}) for u in range(20)]
        fitted = fit(rides, Layout(cfg, gradient=False))
        theta = fitted["coefficients"]["surface"]
        self.assertGreater(theta[5], .4)
        self.assertLessEqual(theta[5], np.log(2)+1e-9)
        self.assertEqual(fitted["training_riders"], 20)

    def test_replay_forecasts_do_not_use_future_elapsed_times(self):
        cfg = Config(initial_pace=(3.,)*9, target_km=(1., 3.))
        values = np.array([[.1]*100, [.3]*100, [0.]*100, [0.]*100], dtype=np.float32)
        matched = {key: [{"surface": {"gravel": 1}}]*100 for key in ("offline", "causal")}
        fitted = {"coefficients": {name: [0.]*size for name, size in MODELS.items()}}
        import csv
        def replay(v, root):
            rows = [(dict(user=1, ride=1, sport="bike"), v, matched)]
            with patch.object(surface_replay, "OUTPUT", root), patch.object(surface_replay, "config", return_value=cfg), \
                    patch.object(surface_replay, "rides", return_value=iter(rows)):
                surface_replay.run("test", fitted)
            with (root / "test.csv").open() as f:
                return list(csv.DictReader(f))
        with tempfile.TemporaryDirectory() as temporary:
            before = replay(values, Path(temporary))
            altered = values.copy()
            altered[1, 40:] = .45
            after = replay(altered, Path(temporary))
        self.assertEqual([r["predicted_minutes"] for r in before], [r["predicted_minutes"] for r in after])
        self.assertNotEqual([r["actual_minutes"] for r in before], [r["actual_minutes"] for r in after])
        for name in MODELS:
            keys = lambda rows: [(r["phase"], r["target_km"], r["status"]) for r in rows if r["model"] == name]
            self.assertEqual(keys(before), [(r["phase"], r["target_km"], r["status"]) for r in before if r["model"] == "gradient"])


if __name__ == "__main__":
    unittest.main()
