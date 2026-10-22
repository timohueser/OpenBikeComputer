"""The water solver on synthetic terrain: a noisy V-valley, lake shores, a confluence, a fall and a dam."""

import math
import unittest

import numpy as np

from tools.trailview import water

# A point at Engelberg in world coordinates, and the metres of a world unit there.
X0, Y0 = (8.4 + 180) / 360, 0.5 - math.log(math.tan(math.pi / 4 + math.radians(46.8) / 2)) / (2 * math.pi)
SCALE = float(water.metres(Y0))


def at(east, south):
    """The world point `east` and `south` metres from the origin."""
    return (X0 + east / SCALE, Y0 + south / SCALE)


class Terrain:
    """Heights from a function of metres east and south of the origin."""

    top = 13

    def __init__(self, height):
        self.height = height

    def heights(self, x, y):
        return self.height((np.asarray(x) - X0) * SCALE, (np.asarray(y) - Y0) * SCALE).astype(np.float32)


def falls(z):
    return all(b <= a + 1e-9 for a, b in zip(z, z[1:]))


class Rivers(unittest.TestCase):
    def test_a_river_in_a_noisy_v_valley_falls_with_the_valley_and_stays_at_its_bottom(self):
        # A valley that runs east and falls 5 %, with sides of 30 % and a metre of noise.
        noise = np.random.default_rng(1).normal(0, 0.5, 4000)
        valley = Terrain(lambda e, s: 1000 - 0.05 * e + 0.3 * np.abs(s) + noise[np.clip((e / 0.5).astype(int), 0, 3999)])
        points = [at(e, 3 * math.sin(e / 40)) for e in range(0, 1801, 60)]
        z = water.solve([("river", {"waterway": "river"}, [(False, points, None)])], valley)[0][0][0]
        self.assertTrue(falls(z))
        bottom = [1000 - 0.05 * e for e in range(0, 1801, 60)]
        self.assertLess(max(abs(a - b) for a, b in zip(z, bottom)), 2.0)

    def test_a_way_drawn_uphill_turns_round(self):
        slope = Terrain(lambda e, s: 1000 + 0.02 * e + 0 * s)
        points = [at(e, 0) for e in range(0, 1001, 100)]
        heights, metrics = water.solve([("stream", {"waterway": "stream"}, [(False, points, None)])], slope)
        self.assertEqual(points[0], at(1000, 0), "the way runs downstream now")
        self.assertTrue(falls(heights[0][0]))
        self.assertEqual(metrics["turned"], 1)

    def test_a_confluence_has_one_level_and_nothing_rises_through_it(self):
        # Two streams from the north-west and the south-west meet at the origin; the river goes on east.
        plain = Terrain(lambda e, s: 900 - 0.01 * e + 0.02 * np.abs(s))
        north = [at(-400, -300), at(-200, -150), at(0, 0)]
        south = [at(-400, 300), at(-200, 150), at(0, 0)]
        river = [at(0, 0), at(300, 0), at(600, 0)]
        features = [("stream", {"waterway": "stream"}, [(False, north, None)]), ("stream", {"waterway": "stream"}, [(False, south, None)]),
                    ("river", {"waterway": "river"}, [(False, river, None)])]
        (a,), (b,), (c,) = water.solve(features, plain)[0]
        self.assertTrue(falls(a) and falls(b) and falls(c))
        self.assertEqual(a[-1], c[0])
        self.assertEqual(b[-1], c[0])

    def test_a_branch_off_a_way_that_runs_on_does_not_rise(self):
        # A river runs east through the origin and drops 3 m 15 m past it, so its smoothing lowers its
        # level at the origin after the reach before it; a canal branches off north there on the flat.
        terrain = Terrain(lambda e, s: 900 - 0.01 * e - 3.0 * (e > 15) - 0.002 * np.abs(s) * (e <= 15))
        river = [at(-400, 0), at(-200, 0), at(0, 0), at(200, 0), at(400, 0)]
        canal = [at(0, 0), at(0, -60), at(0, -120)]
        features = [("river", {"waterway": "river"}, [(False, river, None)]), ("river", {"waterway": "canal"}, [(False, canal, None)])]
        (r,), (c,) = water.solve(features, terrain)[0]
        self.assertTrue(falls(r) and falls(c))
        self.assertEqual(c[0], r[2])

    def test_a_loop_of_channels_keeps_its_fall_and_nothing_rises_through_it_in_any_order(self):
        # A loop P → Q → P around an island, with Q its low point: "back" is drawn up a slope too gentle
        # to turn, and so is an outflow from Q.
        terrain = Terrain(lambda e, s: 1000 - 0.01 * np.minimum(e, 400) + 0.01 * np.maximum(e - 400, 0) + 0 * s)
        ways = {"there": [at(0, 0), at(200, 50), at(400, 0)], "back": [at(400, 0), at(200, -50), at(0, 0)],
                "out": [at(400, 0), at(600, 0), at(800, 0)]}
        solved = []
        for order in (["out", "there", "back"], ["there", "back", "out"]):
            heights = water.solve([("river", {"waterway": "river"}, [(False, list(ways[name]), None)]) for name in order], terrain)[0]
            solved.append({name: z for name, (z,) in zip(order, heights)})
        self.assertEqual(solved[0], solved[1])
        z = solved[0]
        self.assertTrue(all(falls(v) for v in z.values()))
        # The channel down the valley keeps its fall; the one drawn back up it stays under Q.
        self.assertGreater(z["there"][0], z["there"][-1] + 3)
        self.assertEqual(z["back"][0], z["there"][-1])
        self.assertEqual(z["out"][0], z["there"][-1])

    def test_a_fall_keeps_its_step_and_the_pools_above_and_below_stay_flat(self):
        # Two flat pools, 10 m apart, with a fall between them at 500 m.
        step = Terrain(lambda e, s: np.where(e < 500, 1010.0, 1000.0) + 0 * s)
        points = [at(e, 0) for e in range(0, 1001, 50)]
        features = [("river", {"waterway": "river"}, [(False, points, None)]), ("waterfall", {}, [(False, [at(500, 0)], None)])]
        z = water.solve(features, step)[0][0][0]
        self.assertTrue(falls(z))
        upper, lower = z[points.index(at(450, 0))], z[points.index(at(550, 0))]
        self.assertGreater(upper, 1009.5)
        self.assertLess(lower, 1000.5)


class Lakes(unittest.TestCase):
    SQUARE = [at(0, 0), at(400, 0), at(400, 400), at(0, 400)]

    def level(self, terrain, tags=None):
        return water.solve([("lake", tags or {}, [(True, list(self.SQUARE), None)])], terrain)[0][0][0][0]

    def test_a_flat_lake_takes_the_median_of_its_interior(self):
        self.assertAlmostEqual(self.level(Terrain(lambda e, s: 700.0 + 0 * e)), 700.0, places=3)

    def test_the_interior_of_a_lake_leaves_out_its_island(self):
        # The island's ring is closed: its last vertex repeats the first.
        island = [at(40, 40), at(360, 40), at(360, 360), at(40, 360), at(40, 40)]
        terrain = Terrain(lambda e, s: np.where((e > 40) & (e < 360) & (s > 40) & (s < 360), 760.0, 700.0))
        h = water.interior([(True, list(self.SQUARE), None), (False, island, None)], terrain)
        self.assertTrue(h.size)
        self.assertTrue(np.all(h == 700.0))

    def test_a_sloped_lake_takes_a_low_quantile_of_its_shore_and_an_ele_tag_near_it(self):
        # The terrain inside a lake that a DTM did not flatten: it falls 2 % to the south-east.
        sloped = Terrain(lambda e, s: 708.0 - 0.01 * (e + s))
        level = self.level(sloped)
        self.assertLess(level, 702.0)
        self.assertGreater(level, 700.0)
        self.assertEqual(self.level(sloped, {"ele": f"{level + 1:.1f}"}), round(level + 1, 1))
        self.assertAlmostEqual(self.level(sloped, {"ele": "690"}), level)
        self.assertLess(self.level(sloped, {"water": "pond"}), level)

    def test_a_dam_reservoir_takes_the_terrain_at_its_outline_and_its_outflow_stays_below_it(self):
        # A reservoir held by a dam at its east side; the terrain inside has dropped with the water.
        valley = Terrain(lambda e, s: np.where((e > 5) & (e < 395) & (s > 5) & (s < 395), 1850.0, 1900.0) - np.where(e >= 400, 0.1 * (e - 400), 0))
        outflow = [at(400, 200), at(600, 200), at(800, 200)]
        features = [("lake", {"landuse": "reservoir"}, [(True, list(self.SQUARE), None)]), ("dam", {}, [(False, [at(400, 150), at(400, 250)], None)]),
                    ("river", {"waterway": "river"}, [(False, outflow, None)])]
        heights, metrics = water.solve(features, valley)
        level, river = heights[0][0][0], heights[2][0]
        self.assertGreater(level, 1890.0, "the full level, at the outline")
        self.assertTrue(falls(river))
        self.assertLessEqual(river[0], level)
        self.assertEqual(metrics["lakes"], 1)


if __name__ == "__main__":
    unittest.main()
