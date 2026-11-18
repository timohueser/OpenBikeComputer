"""The routing pack of a grid release."""

import json
from pathlib import Path
import tempfile
import unittest

from tools import planner_grid_pack as pack


class RoutingPack(unittest.TestCase):
    def test_the_route_files_keep_their_subfolders_under_routes_tiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "blocks.json").write_text(json.dumps({"source": "s", "archives": []}))
            (root / "catalog.json").write_text(json.dumps({"source": "s", "cells": []}))
            routes = ["routes/9-267-177.json", "routes/trails/7-66-45.json", "routes/long/123.json"]
            layers = {"planner/routing": {"blocks/blocks.json": str(root / "blocks.json"),
                                          "blocks/catalog.json": str(root / "catalog.json"),
                                          **{name: f"/source/{name}" for name in routes}}}
            files, _ = pack.routing(layers)
        self.assertEqual({name: source for name, source in files.items() if name.startswith("routes/")}, {
            "routes/tiles/9-267-177.json": "/source/routes/9-267-177.json",
            "routes/tiles/trails/7-66-45.json": "/source/routes/trails/7-66-45.json",
            "routes/tiles/long/123.json": "/source/routes/long/123.json",
        })


if __name__ == "__main__":
    unittest.main()
