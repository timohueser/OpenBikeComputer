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

    def test_a_steep_rise_by_a_station_gets_a_pylon_that_keeps_the_cable_over_the_ground(self):
        # The ground rises 12 m in the first 20 m from the station, then gently: the worst point lies too near the station for a pylon.
        rise = Terrain(lambda e, s: 1000 + np.where(e < 20, 0.6 * e, 12 + 0.1 * (e - 20)) + 0 * s)
        out = lifts.pylons({"aerialway": "chair_lift"}, [at(0, 0), at(170, 0)], rise)
        rig, e = lifts.RIGS["chair_lift"], np.linspace(0, 170, 341)
        heights = rise.heights(*np.array(out).T) + np.array([rig["station"]] + [rig["pylon"]] * (len(out) - 2) + [rig["station"]])
        stops = [east(p) for p in out]
        span = np.searchsorted(stops, e, side="right").clip(1, len(out) - 1) - 1
        u = (e - np.take(stops, span)) / np.diff(stops)[span]
        cable = heights[span] + (heights[span + 1] - heights[span]) * u - 4 * rig["sag"] * np.diff(stops)[span] * u * (1 - u)
        self.assertGreater(len(out), 2)
        self.assertGreater(float(np.min(cable - rise.heights(*np.array([at(x, 0) for x in e]).T))), 0)

    def test_unknown_terrain_adds_no_pylon(self):
        unknown = Terrain(lambda e, s: np.full(np.shape(e), np.nan))
        points = [at(0, 0), at(100, 0)]
        self.assertEqual(lifts.pylons({"aerialway": "drag_lift"}, points, unknown), points)


class Stations(unittest.TestCase):
    def test_a_way_through_the_end_of_another_lift_splits_there_and_one_station_stands(self):
        a, m, b, c = at(0, 0), at(300, 0), at(600, 0), at(300, 300)
        out, _, metrics = lifts.rigged([({"aerialway": "gondola"}, [a, m, b]), ({"aerialway": "chair_lift"}, [m, c])], [])
        self.assertEqual([points for _, points in out], [[a, m], [m, b], [m, c]])
        self.assertEqual([tags.get("stations") for tags, _ in out], [None, "end", "end"])
        self.assertEqual(metrics["shared_ends"], 2)

    def test_the_station_replaces_the_building_that_holds_its_end_and_other_buildings_stay(self):
        def square(e, s):
            return [np.array([at(e - 10, s - 10), at(e + 10, s - 10), at(e + 10, s + 10), at(e - 10, s + 10)])]
        out, stations, _ = lifts.rigged([({"aerialway": "drag_lift"}, [at(0, 0), at(100, 0)])], [square(200, 0), square(0, 0)])
        self.assertNotIn("stations", out[0][0])
        self.assertEqual(stations, {1})


if __name__ == "__main__":
    unittest.main()
