"""The road solver: a road follows a noisy slope without its noise, a hairpin climbs within the grade of its
class, a side road meets the through road flat, a short link of a lower class does not move its roads, a
ford lies under the water, a low short bridge is a culvert, a bridge past unknown terrain has no height, and of
two ways on the ground the one of the higher layer passes over, but not over one indoors."""

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

        (solved,), _, _, _ = roads.solve([road("road", "residential", line((0, 0), (500, 0), step=50))], Terrain(height))
        along, z = profile(solved[0])
        self.assertLess(np.max(np.abs(z - (1000 + 0.10 * along))), 0.15)
        self.assertLessEqual(np.max(np.abs(np.diff(z) / np.diff(along))), 0.20 + 1e-4)

    def test_a_hairpin_climbs_within_the_grade_of_its_class(self):
        # A hillside of 30 %, and a tertiary road (15 %) that turns back on it: the bend alone would climb at 20 %.
        terrain = Terrain(lambda east, north: 1000 + 0.30 * north)
        bend = [(300, 36), (312, 41), (316, 46), (312, 51), (300, 56)]
        (solved,), _, _, _ = roads.solve([road("road", "tertiary", line((0, 0), *bend, (0, 92), step=20))], terrain)
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
        (w, e, s), _, _, metrics = roads.solve([west, east, side], terrain)
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

    def test_a_short_link_of_a_lower_class_does_not_move_its_roads(self):
        # Two primary roads on the two levels of a terrace step of 2.4 m, and a residential link between
        # them, steeper than its class: its grade, its smoothness and its two ends give way.
        for gap in (4.5, 8, 9.5):
            with self.subTest(gap=gap):
                terrain = Terrain(lambda east, north: 1000 + 2.4 * (north > gap / 2))
                lower = road("main-road", "primary", line((-100, 0), (0, 0), (100, 0), step=20))
                upper = road("main-road", "primary", line((-100, gap), (0, gap), (100, gap), step=20))
                link = road("road", "residential", line((0, 0), (0, gap)))
                (lo, up, _), _, _, _ = roads.solve([lower, upper, link], terrain)
                self.assertLess(abs(dict(zip(lo[0][1], lo[0][2]))[at(0, 0)] - 1000), 0.2)
                self.assertLess(abs(dict(zip(up[0][1], up[0][2]))[at(0, gap)] - 1002.4), 0.2)

    def test_a_ford_lies_under_the_water_and_a_low_short_bridge_is_a_culvert(self):
        def height(east, north):
            gully = np.where(np.abs(east - 115) < 10, 8.0, 0.0)
            return np.where((east > 251) & (north > 60), np.nan, 1000 - gully + 0 * north)

        crossing = at(50, 0)
        ford = road("road", "track", line((0, 0), (50, 0), (100, 0)))
        culvert = road("road", "track", line((30, 50), (36, 50)), bridge="yes")
        bridge = road("road", "track", line((100, 0), (130, 0)), bridge="yes")
        # A way tagged as a ford that crosses a stream between two of its vertices, 10 m along it.
        tagged = road("road", "track", line((60, 30), (66, 30), (74, 30), (84, 30)), ford="yes")
        # A bridge that only roads in unknown terrain join to the known terrain.
        known = road("road", "track", line((200, 80), (250, 80)))
        dark = [road("road", "track", line((250, 80), (300, 80))), road("road", "track", line((320, 80), (350, 80)))]
        unknown = road("road", "track", line((300, 80), (320, 80)), bridge="yes")
        streams = [([at(50, -10), crossing, at(50, 10)], [999.1, 999.0, 998.9]), ([at(70, 20), at(70, 40)], [998.6, 998.4])]
        solved, culverts, _, _ = roads.solve([ford, culvert, bridge, tagged, known, *dark, unknown], Terrain(height), streams)
        self.assertAlmostEqual(dict(zip(solved[0][0][1], solved[0][0][2]))[crossing], 999.0 - roads.FORD_M, delta=0.01)
        along, z = profile(solved[3][0])
        self.assertAlmostEqual(np.interp(10, along, z), 998.5 - roads.FORD_M, delta=0.01)
        self.assertEqual(culverts, {1})
        along, z = profile(solved[2][0])
        # The bridge runs straight over the gully between its ends.
        self.assertLess(np.max(np.abs(z - np.interp(along, [along[0], along[-1]], [z[0], z[-1]]))), 0.05)
        self.assertEqual(solved[7][0][2], [None] * len(solved[7][0][1]))

    def test_a_tunnel_that_ends_in_the_mountain_is_level_from_the_road_that_it_leaves(self):
        terrain = Terrain(lambda east, north: 1000 + 0.3 * np.maximum(east, 0))
        features = [road("road", "residential", line((0, -300), (0, 300), step=20)),
                    road("tunnel", "footway", line((0, 0), (400, 0), step=5), tunnel="yes")]
        out, *_ = roads.solve(features, terrain)
        _, z = profile(out[1][0])
        self.assertLess(np.max(np.abs(z - z[0])), 1e-3)

    def test_a_way_on_the_ground_of_a_higher_layer_passes_over_and_one_indoors_stays_on_the_terrain(self):
        terrain = Terrain(lambda east, north: 1000 + 0 * east)
        street = road("road", "residential", line((-100, 0), (100, 0), step=20))
        for tags, rise in (({"layer": "1"}, roads.CLEAR_ROAD_M + roads.DECK_PATH_M), ({"layer": "-1", "indoor": "yes"}, 0.0)):
            with self.subTest(**tags):
                corridor = road("path", "footway", line((5, -50), (5, 50)), **tags)
                (s, c), _, _, metrics = roads.solve([street, corridor], terrain)
                over = np.interp(50, *profile(c[0])) - np.interp(105, *profile(s[0]))
                self.assertEqual(metrics["passes"], 1 if rise else 0)
                self.assertGreaterEqual(over, rise - roads.KEEP_M)
                self.assertLess(over, rise + 0.5)


class AreasTest(unittest.TestCase):
    def test_a_network_over_four_areas_solves_by_area_as_it_solves_whole(self):
        n = 1 << roads.AREA_ZOOM
        corner = ((math.floor(X0 * n) + 1) / n - X0) / UNIT, (Y0 - math.floor(Y0 * n) / n) / UNIT
        near = lambda *points: [(corner[0] + east, corner[1] + north) for east, north in points]
        noise = np.random.default_rng(3).normal(0, 0.4, 20000)
        terrain = Terrain(lambda east, north: 1000 + 0.15 * east - 0.03 * north + noise[(np.abs(east + 3 * north) / 0.5).astype(int) % 20000])
        features = [
            road("main-road", "secondary", line(*near((-600, -400), (0, 0), (600, 300)), step=40)),
            road("road", "residential", line(*near((0, 0), (-200, 300), (-300, 500)), step=30)),
            road("road", "track", line(*near((200, -300), (220, -200)), step=10), bridge="yes"),
            road("road", "track", line(*near((-500, 200), (-100, 200)), step=50)),
            road("tunnel", "residential", line(*near((-300, -100), (300, -100)), step=60), tunnel="yes"),
            road("road", "track", line(*near((40, 100), (46, 100))), bridge="yes"),
        ]
        stream = [at(*point) for point in near((210, -260), (180, -240))]
        rivers = [(stream, [990.0, 989.5])]
        self.assertEqual(len(roads.Region(features).areas), 4)
        whole = roads.solve(features, terrain, rivers)
        apart = roads.solve_by_area(features, terrain, rivers, workers=2)
        self.assertEqual(whole[:3], apart[:3])
        self.assertEqual({k: v for k, v in whole[3].items() if k != "residual_p99_m"}, {k: v for k, v in apart[3].items() if k != "residual_p99_m"})


if __name__ == "__main__":
    unittest.main()
