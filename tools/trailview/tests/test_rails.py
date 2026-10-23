"""The tracks: the two tracks of a double line lie at one height but a track on a ramp beside another does not, a road
bridge clears the contact wire of the track under it and a road tunnel with an old `railway` tag stays a road, a mast
stands outside a double track, none beside a middle track and none near another, and a platform stands over its rail."""

import unittest

import numpy as np

from tools.trailview import rails, roads, structures
from tools.trailview.tests.test_roads import UNIT, Terrain, X0, Y0, at, line, profile, road


def track(points, **tags):
    return ("railway", {"railway": "rail", "electrified": "contact_line", **tags}, [(False, points, None)])


class RailTest(unittest.TestCase):
    def test_the_tracks_of_a_double_line_on_noisy_ground_lie_at_one_height(self):
        noisy = Terrain(lambda e, n: 500 + 0.02 * e + 0.2 * np.sin(e / 7) * np.sign(n))
        ways = [track(line((0, 2), (400, 2), step=10)), track(line((0, -2), (400, -2), step=10))]
        out, _, _, _ = roads.solve(ways, noisy)
        north, south = profile(out[0][0]), profile(out[1][0])
        self.assertLess(np.max(np.abs(np.interp(south[0], *north) - south[1])), 0.01)

    def test_a_track_on_a_ramp_beside_a_level_track_keeps_its_own_height(self):
        # The embankment north of the level track rises 8 m.
        ramp = Terrain(lambda e, n: 500 + np.where(n > 2, 8 * np.clip((e - 50) / 300, 0, 1), 0))
        ways = [track(line((0, 4), (400, 4), step=10)), track(line((0, 0), (400, 0), step=10))]
        out, _, _, _ = roads.solve(ways, ramp)
        self.assertGreater(np.interp(380, *profile(out[0][0])), 507.5)

    def test_a_road_bridge_clears_the_track_under_it(self):
        flat = Terrain(lambda e, n: np.full(np.shape(e), 500.0))
        ways = [track(line((-300, 0), (300, 0), step=10)), road("road", "tertiary", line((0, -150), (0, -30))),
                road("road", "tertiary", line((0, -30), (0, 30)), bridge="yes"), road("road", "tertiary", line((0, 30), (0, 150)))]
        out, _, _, metrics = roads.solve(ways, flat)
        self.assertEqual(metrics["clearances_low_cm"], 0)
        along, z = profile(out[2][0])
        rail = np.interp(300, *profile(out[0][0]))
        self.assertGreaterEqual(np.interp(30, along, z) - rail, roads.CLEAR_RAIL_M + roads.DECK_M - 0.01)
        tunnel = {"highway": "tertiary", "tunnel": "yes", "railway": "abandoned"}
        self.assertEqual((roads.class_of(tunnel), structures.bore(tunnel)), ("tertiary", structures.BORE_M))

    def test_masts_stand_outside_a_double_track_and_a_platform_stands_over_its_rail(self):
        flat = lambda north: ({"railway": "rail"}, line((0, north), (200, north)), [500.0, 500.0])
        north, south, middle = flat(4.5), flat(0), flat(-4.5)
        wired = [(tags, points, z, False) for tags, points, z in (north, south, middle)]
        masts = rails.masts(wired, [north, south, middle])
        offsets = sorted({round((Y0 - point[1]) / UNIT, 1) for point, _, _ in masts})
        # The north track's masts stand north of it, the south track's south; the middle track has none.
        self.assertEqual(offsets, [-4.5 - rails.MAST_SIDE_M[False], 4.5 + rails.MAST_SIDE_M[False]])
        self.assertEqual(len(masts), 2 * 4)
        switch = [({"railway": "rail"}, line((0, 50), (4, 50)), [500.0] * 2, False), ({"railway": "rail"}, line((4, 50), (8, 50)), [500.0] * 2, False)]
        self.assertEqual(len(rails.masts(switch, [])), 1)
        platform = [(True, [at(20, 7), at(180, 7), at(180, 13), at(20, 13)], None)]
        (solved,) = rails.platforms([platform], [north])
        self.assertEqual(solved[0][2], [500.0 + rails.PLATFORM_M[False]] * 4)
        self.assertEqual(rails.platforms([[(True, [(X0, Y0 - 500 * UNIT)] * 3, None)]], [north]), [None])


if __name__ == "__main__":
    unittest.main()
