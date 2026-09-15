"""Causality and gap boundaries for the private recorded-motion replay."""

from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from komoot_replay import blocks, configuration, history_indices, new_device, replay
from model import Config, Layout


class KomootReplayTests(unittest.TestCase):
    def setUp(self):
        self.cfg = Config()
        self.reference = [np.linspace(-.5, .5, 64)]*6

    def values(self):
        return np.array([np.full(80, .1), np.ones(80), np.zeros(80),
                         np.ones(80), np.tile([0., 1.], 40), np.zeros(80)])

    def test_future_motion_cannot_change_issued_predictions_or_ranges(self):
        values = self.values()
        changed = values.copy()
        changed[1, 40:] *= 2
        for mode in ("baseline", "gradient"):
            devices = [new_device(self.cfg, self.reference, mode) for _ in range(2)]
            a, b = [replay(v, self.cfg, d) for v, d in zip((values, changed), devices)]
            for x, y in zip(a[:4], b[:4]):
                for field in ("predicted_minutes", "low_minutes", "high_minutes", "theta", "coefficient"):
                    self.assertEqual(x[field], y[field])
            self.assertNotEqual(a[0]["actual_minutes"], b[0]["actual_minutes"])
            self.assertTrue(all(x["theta"] == 0 and x["coefficient"] == 0 for x in a))
            self.assertGreater(devices[0][0].theta, 0)
            if mode == "baseline":
                self.assertLessEqual(int(devices[0][2].count.max()), 1)

    def test_zero_candidate_matches_baseline_during_first_ride(self):
        rows = [replay(self.values(), self.cfg, new_device(self.cfg, self.reference, m))
                for m in ("baseline", "gradient")]
        self.assertEqual([r["predicted_minutes"] for r in rows[0]],
                         [r["predicted_minutes"] for r in rows[1]])

    def test_unknown_event_resets_live_without_erasing_personal_history(self):
        values = self.values()
        values = np.insert(values, 40, [0, 0, 0, 0, 0, 1], axis=1)
        device = new_device(self.cfg, self.reference, "baseline")
        device[0].state[0] = .2
        result = replay(values, self.cfg, device)
        at30, at40 = [next(r for r in result if r["checkpoint"] == c) for c in (30, 40)]
        self.assertGreater(at30["live_log"], 0)
        self.assertEqual(at40["live_log"], 0)
        self.assertAlmostEqual(at40["theta"], .2, places=6)
        self.assertEqual(at40["resets_so_far"], 1)

    def test_blocks_conserve_accepted_route_and_never_cross_a_pause_or_gap(self):
        raw = np.array([[.005, .005, 0, .005, .005, 0, .02],
                        [.1, .1, 0, .1, .1, 0, .2], np.zeros(7)])
        data = dict(raw=raw, reset=np.array([0, 0, 1, 0, 0, 1, 0], bool),
                    learn=np.array([1, 0, 0, 1, 1, 0, 0], bool),
                    unknown_minutes=np.array([0, 0, 0, 0, 0, 5, 0]))
        b = blocks(data, self.cfg, .2)
        np.testing.assert_allclose(b[:2].sum(axis=1), raw[:2].sum(axis=1))
        np.testing.assert_allclose(b[0], [.01, .01, 0, .02])
        np.testing.assert_array_equal(b[3], [0, 1, 0, 0])
        np.testing.assert_array_equal(b[5], [0, 0, 1, 0])
        expected = raw[0] @ np.exp(Layout(self.cfg, gradient=False).initial_log(raw[2])+.2)
        self.assertAlmostEqual(float(b[0] @ np.exp(b[2])), expected)

    def test_ineligible_ride_cannot_update_persistent_state_or_ranges(self):
        device = new_device(self.cfg, self.reference, "baseline")
        before = device[0].state.copy()
        replay(self.values(), self.cfg, device, train=False, calibrate=False)
        np.testing.assert_array_equal(device[0].state, before)
        self.assertEqual(device[2].count.sum(), 0)
        replay(self.values(), self.cfg, device, train=True, calibrate=False)
        self.assertGreater(device[0].theta, 0)
        self.assertEqual(device[2].count.sum(), 0)

    def test_both_imported_bike_labels_resolve_to_frozen_defaults(self):
        cfg, offsets, reference = configuration()
        self.assertEqual(set(offsets), {"mtb", "other"})
        self.assertGreater(offsets["mtb"], offsets["other"])
        self.assertEqual(len(reference), 6)

    def test_history_ignores_current_future_and_ineligible_rides(self):
        rides = [dict(distance_km=d, moving_minutes=t) for d, t in [(40, 60), (100, 4), (80, 90), (200, 100)]]
        self.assertEqual(history_indices(rides, 3, 0), ([], 0))
        self.assertEqual(history_indices(rides, 3, 50), ([2], 80))
        self.assertEqual(history_indices(rides, 3, 100), ([0, 2], 120))


if __name__ == "__main__":
    unittest.main()
