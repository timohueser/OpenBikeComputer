"""The tracks: a road tunnel with an old `railway` tag stays a road, a mast stands outside a double track, none beside a
middle track and none near another, and a platform stands over its rail. The road solver's tests of the tracks are in
`host/obc-trailview-features`."""

import math
import unittest

from tools.trailview import rails, roads, structures
from tools.trailview.profiles import metres

# A point at Engelberg in world units, and world units per metre there.
X0, Y0 = (8.40 + 180) / 360, (1 - math.asinh(math.tan(math.radians(46.82))) / math.pi) / 2
UNIT = 1 / float(metres(Y0))


def at(east, north):
    return (X0 + east * UNIT, Y0 - north * UNIT)


def line(*points):
    return [at(*point) for point in points]


class RailTest(unittest.TestCase):
    def test_a_road_tunnel_with_an_old_railway_tag_stays_a_road(self):
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
