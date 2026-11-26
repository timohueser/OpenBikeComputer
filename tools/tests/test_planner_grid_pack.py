"""The routing pack of a grid release."""

import unittest

from tools import planner_grid_pack as pack


class RoutingPack(unittest.TestCase):
    def test_the_route_files_keep_their_subfolders_under_routes_tiles(self):
        routes = ["routes/9-267-177.json", "routes/trails/7-66-45.json", "routes/long/123.json"]
        layers = {"planner/routing": {"cells/package.json": "/source/cells/package.json",
                                      **{name: f"/source/{name}" for name in routes}}}
        self.assertEqual(pack.routing(layers), {
            "routes/tiles/9-267-177.json": "/source/routes/9-267-177.json",
            "routes/tiles/trails/7-66-45.json": "/source/routes/trails/7-66-45.json",
            "routes/tiles/long/123.json": "/source/routes/long/123.json",
        })


if __name__ == "__main__":
    unittest.main()
