"""The public metadata of a grid release."""

import json
import unittest

from tools import planner_runtime as runtime


class PublicMetadata(unittest.TestCase):
    def test_the_packs_of_a_base_square_share_its_block_index_and_other_files_keep_a_pointer(self):
        def item(n):
            return {"bytes": n, "sha256": str(n) * 64, "transport": {"bytes": n, "sha256": str(n) * 64, "encoding": "identity"}}

        # Base square 9-268-180 is split: its zoom 9 tile, a zoom 10 child and a zoom 11 grandchild.
        files = {"maps/tiles/basemap/9-268-180.pmtiles": item(1), "maps/tiles/basemap/10-537-361.pmtiles": item(2),
                 "maps/tiles/basemap/11-1072-720.pmtiles": item(3), "maps/tiles/terrain/13/10-536-360.pmtiles": item(4),
                 "maps/tiles/basemap/8-134-90.pmtiles": item(5), "maps/tiles/basemap/3-4-2.pmtiles": item(6),
                 "routes/tiles/9-268-178.json": item(7), "maps/basemap.json": item(8),
                 "maps/assets/fonts/Noto Sans Regular/0-255.pbf": item(9), "search/central.grid.json": item(10)}
        public = runtime.public_metadata({"grid": {"format": 3, "zoom": 9}, "files": files})
        read = {name: json.loads(data) for name, data in public.items()}
        pointer = lambda n: {"bytes": n, "sha256": str(n) * 64, "encoding": "identity", "decoded_bytes": n}
        self.assertEqual(read, {
            "public/grid.json": {"format": 4},
            "public/maps/tiles/basemap/7-67-45.json": {"9-268-180.pmtiles": pointer(1), "10-537-361.pmtiles": pointer(2),
                                                       "11-1072-720.pmtiles": pointer(3)},
            "public/maps/tiles/terrain/13/7-67-45.json": {"10-536-360.pmtiles": pointer(4)},
            "public/maps/tiles/basemap/5-16-11.json": {"8-134-90.pmtiles": pointer(5)},
            "public/maps/tiles/basemap/0-0-0.json": {"3-4-2.pmtiles": pointer(6)},
            "public/routes/tiles/5-16-11.json": {"9-268-178.json": pointer(7)},
            "public/maps/basemap.json.json": pointer(8),
            "public/maps/assets/fonts/Noto Sans Regular/0-255.pbf.json": pointer(9),
        })

    def test_a_block_index_that_the_tiles_worker_cannot_read_is_an_error(self):
        # 256 cells of one block, each with a long name.
        files = {f"maps/tiles/basemap/4-{x}-{y}.{'p' * 200}": {"bytes": 1, "sha256": "a" * 64,
                 "transport": {"bytes": 999_999_999, "sha256": "a" * 64, "encoding": "identity"}}
                 for x in range(16) for y in range(16)}
        with self.assertRaisesRegex(ValueError, "exceeds"):
            runtime.public_metadata({"grid": {"format": 3, "zoom": 9}, "files": files})


if __name__ == "__main__":
    unittest.main()
