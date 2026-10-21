"""The road solver: a road follows a noisy slope without its noise, a hairpin climbs within the grade of its
class, a side road meets the through road flat, a ford lies under the water and a low short bridge is a
culvert."""

import math
import unittest

import numpy as np

from tools.trailview import roads
from tools.trailview.profiles import metres

# A point at Engelberg in world units, and world units per metre there.
X0, Y0 = (8.40 + 180) / 360, (1 - math.asinh(math.tan(math.radians(46.82))) / math.pi) / 2
UNIT = 1 / float(metres(Y0))


class Terrain:
    """The terrain as a function of metres east and north of (X0, Y0)."""

    def __init__(self, height):
        self.height = height

    def heights(self, x, y):
        return self.height((np.asarray(x) - X0) / UNIT, (Y0 - np.asarray(y)) / UNIT)


def at(east, north):
    return (X0 + east * UNIT, Y0 - north * UNIT)


def line(*points, step=None):
    """World points through (east, north) corners, with a vertex every `step` metres between them."""
    out = []
    for (e0, n0), (e1, n1) in zip(points, points[1:]):
        count = max(1, round(math.hypot(e1 - e0, n1 - n0) / step)) if step else 1
        out += [at(e0 + (e1 - e0) * i / count, n0 + (n1 - n0) * i / count) for i in range(count)]
    return out + [at(*points[-1])]


def road(kind, highway, points, **tags):
    return (kind, {"highway": highway, **tags}, [(False, points, None)])


def profile(part):
    """Metres along a solved part and its heights."""
    _, points, z = part
    xy = np.asarray(points)
    along = np.concatenate([[0], np.cumsum(np.hypot(*np.diff(xy, axis=0).T) / UNIT)])
    return along, np.asarray(z, np.float64)


class RoadSolverTest(unittest.TestCase):
    def test_a_road_on_a_noisy_slope_follows_the_slope_and_not_the_noise(self):
        def height(east, north):
            spike = np.where(np.abs(east - 250) < 2, 3.0, 0.0)
            return 1000 + 0.10 * east + 0.3 * np.sin(east * 1.7) + spike

        (solved,), _, _ = roads.solve([road("road", "residential", line((0, 0), (500, 0), step=50))], Terrain(height))
        along, z = profile(solved[0])
        self.assertLess(np.max(np.abs(z - (1000 + 0.10 * along))), 0.15)
        self.assertLessEqual(np.max(np.abs(np.diff(z) / np.diff(along))), 0.20 + 1e-4)

    def test_a_hairpin_climbs_within_the_grade_of_its_class(self):
        # A hillside of 30 %, and a tertiary road (15 %) that turns back on it: the bend alone would climb at 20 %.
        terrain = Terrain(lambda east, north: 1000 + 0.30 * north)
        bend = [(300, 36), (312, 41), (316, 46), (312, 51), (300, 56)]
        (solved,), _, _ = roads.solve([road("road", "tertiary", line((0, 0), *bend, (0, 92), step=20))], terrain)
        along, z = profile(solved[0])
        grades = np.diff(z) / np.diff(along)
        self.assertLessEqual(np.max(grades), 0.15 + 1e-4)
        self.assertGreater(np.min(grades), 0)
        self.assertLess(abs(z[0] - 1000), 0.5)
        self.assertLess(abs(z[-1] - (1000 + 0.30 * 92)), 0.5)

    def test_a_side_road_meets_the_through_road_flat_and_on_one_height(self):
        # The through road climbs east at 2 %, and from the junction on at 10 %, on a hillside that rises
        # north at 15 %; OSM splits it at the junction.
        terrain = Terrain(lambda east, north: 1000 + 0.02 * east + 0.08 * np.maximum(east, 0) + 0.15 * north)
        west = road("main-road", "secondary", line((-100, 0), (0, 0), step=20))
        east = road("main-road", "secondary", line((0, 0), (100, 0), step=20))
        side = road("road", "residential", line((0, 0), (0, 100), step=25))
        (w, e, s), _, metrics = roads.solve([west, east, side], terrain)
        junction = at(0, 0)
        heights = [dict(zip(points, z))[junction] for _, points, z in (w[0], e[0], s[0])]
        self.assertAlmostEqual(max(heights) - min(heights), 0, places=9)
        along, z = profile(s[0])
        # The side road's first metres lie on the through road's surface, which is level across it.
        approach = np.flatnonzero(np.isclose(along, roads.APPROACH_M, atol=0.01))
        self.assertEqual(len(approach), 1)
        self.assertLess(abs(z[approach[0]] - z[0]), 0.01)
        self.assertGreater(metrics["approaches"], 0)
        # The through road runs on smoothly from one way into the other: the kink of the terrain spreads.
        (along_w, z_w), (along_e, z_e) = profile(w[0]), profile(e[0])
        before, after = (z_w[-1] - z_w[-2]) / (along_w[-1] - along_w[-2]), (z_e[1] - z_e[0]) / along_e[1]
        self.assertLess(abs(after - before), 0.025)

    def test_a_ford_lies_under_the_water_and_a_low_short_bridge_is_a_culvert(self):
        def height(east, north):
            gully = np.where(np.abs(east - 115) < 10, 8.0, 0.0)
            return 1000 - gully + 0 * north

        crossing = at(50, 0)
        ford = road("road", "track", line((0, 0), (50, 0), (100, 0)))
        culvert = road("road", "track", line((30, 50), (36, 50)), bridge="yes")
        bridge = road("road", "track", line((100, 0), (130, 0)), bridge="yes")
        stream = ([at(50, -10), crossing, at(50, 10)], [999.1, 999.0, 998.9])
        solved, culverts, _ = roads.solve([ford, culvert, bridge], Terrain(height), [stream])
        self.assertAlmostEqual(dict(zip(solved[0][0][1], solved[0][0][2]))[crossing], 999.0 - roads.FORD_M, delta=0.01)
        self.assertEqual(culverts, {1})
        along, z = profile(solved[2][0])
        # The bridge runs straight over the gully between its ends.
        self.assertLess(np.max(np.abs(z - np.interp(along, [along[0], along[-1]], [z[0], z[-1]]))), 0.05)


if __name__ == "__main__":
    unittest.main()
