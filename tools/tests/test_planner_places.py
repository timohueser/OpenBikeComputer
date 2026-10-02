"""The places archive holds each rider place of the basemap once, in its coarse tile."""

import gzip
from pathlib import Path
import tempfile
import unittest

from tools import planner_places as places


class PlacesArchive(unittest.TestCase):
    def test_keeps_one_copy_of_each_rider_place_in_its_coarse_tile(self):
        from pmtiles.reader import MmapSource, all_tiles
        from pmtiles.tile import Compression, TileType, zxy_to_tileid
        from pmtiles.writer import write
        # Two neighbouring zoom 14 tiles in Freiburg share the zoom 11 tile 1068/717.
        # The left tile repeats place 3 past its right edge, a little off, as a tile buffer does.
        basemap = {(8547, 5739): [(1, {"kind": "camp_site", "name": "Camp", "name:en": "Camp EN", "min_zoom": "13"}, 100, 200),
                                  (2, {"kind": "parking", "name": "Car park"}, 300, 300),
                                  (3, {"kind": "drinking_water"}, 4107, 51)],
                   (8548, 5739): [(3, {"kind": "drinking_water"}, 10, 50)]}
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / "basemap.pmtiles", Path(directory) / "places.pmtiles"
            with write(source) as writer:
                for (x, y), points in basemap.items():
                    writer.write_tile(zxy_to_tileid(14, x, y), gzip.compress(places.tile(points)))
                writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                                 "min_lon_e7": 78000000, "min_lat_e7": 479000000, "max_lon_e7": 79000000, "max_lat_e7": 480000000,
                                 "center_zoom": 14, "center_lon_e7": 78500000, "center_lat_e7": 479500000}, {"attribution": "OSM"})
            places.derive(source, target)
            with target.open("rb") as stream:
                tiles = list(all_tiles(MmapSource(stream)))
        self.assertEqual([zxy for zxy, _ in tiles], [(11, 1068, 717)])
        self.assertEqual(list(places.pois(gzip.decompress(tiles[0][1]))), [
            (1, {"kind": "camp_site", "name": "Camp", "name:en": "Camp EN"}, 1548, 1561, 4096),
            (3, {"kind": "drinking_water"}, 2049, 1542, 4096)])


if __name__ == "__main__":
    unittest.main()
