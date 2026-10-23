"""The pylons of lifts on synthetic terrain."""

import unittest

import numpy as np

from tools.trailview import lifts
from tools.trailview.tests.test_water import SCALE, X0, Terrain, at


def east(point):
    return (point[0] - X0) * SCALE


class Pylons(unittest.TestCase):
    def test_a_cable_over_a_valley_keeps_its_stations_only(self):
        valley = Terrain(lambda e, s: 1500 - 400 * np.sin(np.pi * np.clip(e, 0, 1000) / 1000) + 0 * s)
        points = [at(0, 0), at(1000, 0)]
        self.assertEqual(lifts.pylons({"aerialway": "cable_car"}, points, valley), points)

    def test_a_knoll_under_a_chair_lift_gets_a_pylon_on_it(self):
        knoll = Terrain(lambda e, s: 1000 + 0.3 * e + 25 * np.exp(-(((e - 70) / 15) ** 2)) + 0 * s)
        out = lifts.pylons({"aerialway": "chair_lift"}, [at(0, 0), at(160, 0)], knoll)
        self.assertEqual(len(out), 3)
        self.assertAlmostEqual(east(out[1]), 70, delta=12)

    def test_a_long_gondola_stands_on_pylons_no_farther_apart_than_its_rig_allows(self):
        slope = Terrain(lambda e, s: 1000 + 0.4 * e + 0 * s)
        out = lifts.pylons({"aerialway": "gondola"}, [at(0, 0), at(2000, 0)], slope)
        spans = [east(b) - east(a) for a, b in zip(out, out[1:])]
        self.assertLessEqual(max(spans), lifts.RIGS["gondola"]["span"] + 1e-6)
        self.assertEqual((out[0], out[-1]), (at(0, 0), at(2000, 0)))

    def test_unknown_terrain_adds_no_pylon(self):
        unknown = Terrain(lambda e, s: np.full(np.shape(e), np.nan))
        points = [at(0, 0), at(100, 0)]
        self.assertEqual(lifts.pylons({"aerialway": "drag_lift"}, points, unknown), points)


if __name__ == "__main__":
    unittest.main()
