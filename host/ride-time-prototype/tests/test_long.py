"""Boundaries and causal replay for the long-ride adapter."""

import sys
from pathlib import Path
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from long_data import duplicate, intervals, match_metadata
from long_replay import blocks, history_start, new_device, ride_replay
from model import Config


class LongRideTests(unittest.TestCase):
    def test_stops_pushing_and_gaps(self):
        seconds = np.arange(7.)
        km = np.array([0, .001, .002, .002, .002, .003, .004])
        altitude = km*10
        raw = intervals(seconds, km, altitude)
        self.assertAlmostEqual(raw[1].sum()*60, 4)
        self.assertAlmostEqual(intervals(seconds, km, altitude, True)[1].sum()*60, 6)
        result = blocks(raw, Config(), 0)
        self.assertAlmostEqual(result[0].sum(), .004)
        self.assertAlmostEqual(result[1].sum()*60, 4)
        for bad_time, bad_km in ((np.array([0., 31]), np.array([0., .02])),
                                 (np.array([0., 1]), np.array([.02, 0.]))):
            with self.assertRaises(ValueError):
                intervals(bad_time, bad_km, np.array([0., 1.]))

    def test_no_future_outcome_or_current_ride_learning_in_prediction(self):
        cfg = Config()
        values = np.array([np.full(80, .1), np.ones(80), np.zeros(80), np.ones(80)])
        reference = [np.linspace(-.5, .5, 64)]*6
        learner, ranges = new_device(cfg, reference)
        original = ride_replay(values, cfg, learner, ranges)
        changed = values.copy()
        changed[1, 40:] *= 2
        learner2, ranges2 = new_device(cfg, reference)
        altered = ride_replay(changed, cfg, learner2, ranges2)
        for a, b in zip(original[:4], altered[:4]):
            self.assertEqual(a["predicted_minutes"], b["predicted_minutes"])
            self.assertEqual(a["high_minutes"], b["high_minutes"])
            self.assertEqual(a["theta"], 0)
        self.assertNotEqual(original[0]["actual_minutes"], altered[0]["actual_minutes"])
        self.assertGreater(learner.theta, 0)
        self.assertLessEqual(int(ranges.count.max()), 1)

    def test_history_uses_only_previous_whole_rides(self):
        rides = [dict(distance_km=d) for d in (40, 80, 200)]
        self.assertEqual(history_start(rides, 2, 0), (2, 0))
        self.assertEqual(history_start(rides, 2, 50), (1, 80))
        self.assertEqual(history_start(rides, 2, 100), (0, 120))

    def test_ambiguous_metadata_and_duplicate_recordings(self):
        records = [dict(date="2007/01/01 10:01:02 UTC"), dict(date="2007/01/01 11:01:02 UTC")]
        self.assertIsNone(match_metadata("2007_01_01_11_01_02.csv", records))
        self.assertEqual(match_metadata("2007_01_01_11_01_02.csv", records[:1]), records[0])
        a = dict(date="2007/01/01", distance_km=100, elapsed_minutes=240)
        self.assertTrue(duplicate(a, dict(a, distance_km=102)))
        self.assertFalse(duplicate(a, dict(a, date="2007/01/02")))


if __name__ == "__main__":
    unittest.main()
