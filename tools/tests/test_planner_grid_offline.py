"""The offline catalog lists the cells, and each cell file lists the packs that an area of the cell takes."""

import json
from pathlib import Path
import tempfile
import unittest

from tools import planner_grid_offline as grid_offline
from tools.tests.test_planner_grid_index import ARCHIVES, OPTIONS, write_indexes


class PlannerGridOffline(unittest.TestCase):
    def test_a_cell_file_lists_the_packs_of_its_geometry_and_the_routing_packs_of_its_archives(self):
        with tempfile.TemporaryDirectory() as temporary:
            layers = write_indexes(Path(temporary))
            indexes = {item["kind"]: item for item in (json.loads(Path(path).read_bytes())
                                                       for selected in layers.values() for path in selected.values())}
        catalog, cells = grid_offline.compose(indexes, OPTIONS)
        self.assertEqual(catalog["cells"], ["9-267-177", "9-268-177"])
        self.assertEqual(set(catalog["assets"]), {"maps/assets/sprites/v4.json", "offline/fonts/Sans.pbf"})
        self.assertEqual(catalog["shared"]["maps/assets/fonts/Sans/0-255.pbf"], "offline/fonts/Sans.pbf")
        shared = {"maps/tiles/places/0-0-0.pmtiles", "maps/tiles/overlays/6-33-22.pmtiles"}
        west, east = cells["9-267-177"], cells["9-268-177"]
        # The roads of the western cell reach the basemap of the cell west of it; terrain adds a ring of
        # tiles; the zoom 13 terrain is TrailView's only.
        self.assertEqual(set(west["packs"]), {*shared, "maps/tiles/basemap/9-266-177.pmtiles",
                                              "maps/tiles/basemap/9-267-177.pmtiles", "maps/tiles/terrain/9-267-177.pmtiles"})
        self.assertEqual(set(east["packs"]), {*shared, "maps/tiles/basemap/9-268-177.pmtiles",
                                              "maps/tiles/terrain/9-267-177.pmtiles", "maps/tiles/terrain/9-269-177.pmtiles"})
        self.assertEqual(west["packs"]["maps/tiles/basemap/9-267-177.pmtiles"],
                         indexes["basemap"]["files"]["maps/tiles/basemap/9-267-177.pmtiles"])
        key = ARCHIVES["9-267-177"]
        self.assertEqual(set(west["routing_packs"]), {f"routing/packs/{key}/pages.bin", f"routing/packs/{key}/pages.idx"})
        self.assertEqual(west["routing"]["archives"], [key])
        self.assertEqual(set(west["files"]), {"search/tiles/pois/9-267-177.sqlite",
                                              "search/tiles/addresses/9-267-177.sqlite", "routes/tiles/9-267-177.json"})


if __name__ == "__main__":
    unittest.main()
