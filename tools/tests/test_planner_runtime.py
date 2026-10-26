"""The public metadata of a grid release."""

import json
import unittest

from tools import planner_runtime as runtime


class PublicMetadata(unittest.TestCase):
    def test_the_packs_of_a_cell_share_the_index_of_its_block_and_other_files_keep_a_pointer(self):
        def item(n):
            return {"bytes": n, "sha256": str(n) * 64, "transport": {"bytes": n, "sha256": str(n) * 64, "encoding": "identity"}}

        files = {"maps/tiles/basemap/11-1071-722.pmtiles": item(1), "maps/tiles/basemap/11-1070-720.pmtiles": item(2),
                 "maps/tiles/terrain/13/11-1071-722.pmtiles": item(3), "maps/tiles/basemap/3-4-2.pmtiles": item(4),
                 "routes/tiles/9-268-178.json": item(5), "maps/basemap.json": item(6),
                 "maps/assets/fonts/Noto Sans Regular/0-255.pbf": item(7), "search/central.grid.json": item(8)}
        public = runtime.public_metadata({"grid": {"format": 2, "zoom": 9, "map_zoom": 11}, "files": files})
        read = {name: json.loads(data) for name, data in public.items()}
        pointer = lambda n: {"bytes": n, "sha256": str(n) * 64, "encoding": "identity", "decoded_bytes": n}
        self.assertEqual(read, {
            "public/grid.json": {"format": 3, "map_zoom": 11},
            "public/maps/tiles/basemap/7-66-45.json": {"11-1071-722.pmtiles": pointer(1), "11-1070-720.pmtiles": pointer(2)},
            "public/maps/tiles/terrain/13/7-66-45.json": {"11-1071-722.pmtiles": pointer(3)},
            "public/maps/tiles/basemap/0-0-0.json": {"3-4-2.pmtiles": pointer(4)},
            "public/routes/tiles/5-16-11.json": {"9-268-178.json": pointer(5)},
            "public/maps/basemap.json.json": pointer(6),
            "public/maps/assets/fonts/Noto Sans Regular/0-255.pbf.json": pointer(7),
        })


if __name__ == "__main__":
    unittest.main()
