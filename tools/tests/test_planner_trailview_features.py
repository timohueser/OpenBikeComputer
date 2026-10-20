"""The kind table, the feature tile codec and the bake of a small OSM file."""

import gzip
import json
import math
import os
from pathlib import Path
import shutil
import tempfile
import unittest

from tools import planner_trailview_features as features

VECTORS = Path(__file__).resolve().parents[2] / "specs/vectors"


class KindTable(unittest.TestCase):
    def test_the_table_is_valid_and_a_broken_entry_is_not(self):
        kinds = features.validate(json.loads(features.TABLE.read_text()))
        entry = kinds[0]
        broken = [{**entry, "geometry": "volume"}, {**entry, "colour": "red"}, {**entry, "match": [{"highway": []}]},
                  {**entry, "keep": ["width", "width"]}, {**entry, "min_zoom": 7}, {**entry, "kind": "Main road"}]
        for table in [[item] for item in broken] + [[entry, entry]]:
            with self.subTest(table=table), self.assertRaises(ValueError):
                features.validate(table)


class Codec(unittest.TestCase):
    def test_the_shared_vectors_round_trip(self):
        decoded = json.loads((VECTORS / "trailview-features.json").read_text())
        encoded = (VECTORS / "trailview-features.bin").read_bytes()
        self.assertEqual(features.encode(decoded), encoded)
        self.assertEqual(features.decode(encoded), decoded)


def location(column, row, u, v):
    """Longitude and latitude of the point (u, v) in tile units of the zoom-14 tile (column, row)."""
    x, y = (column + u / features.EXTENT) / (1 << 14), (row + v / features.EXTENT) / (1 << 14)
    return x * 360 - 180, math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y))))


@unittest.skipUnless(shutil.which(os.environ.get("OBC_OSMIUM", "osmium")) or os.environ.get("CI"),
                     "Prepared Osmium is unavailable")
class Bake(unittest.TestCase):
    # A zoom-14 tile in Engelberg. Each node is (id, u, v) in its tile units.
    COLUMN, ROW = 8574, 5778
    NODES = [(1, 7000, 4000), (2, 9500, 4000), (3, 3000, 3000), (4, 3400, 3000), (5, 3400, 3400), (6, 3000, 3400),
             (7, 3100, 3100), (8, 3100, 3200), (9, 3200, 3200), (10, 3200, 3100), (11, 5000, 5000), (12, 5100, 5000),
             (13, 5100, 5100), (14, 1000, 6000), (15, 1500, 6000), (16, 1000, 7000), (17, 2000, 7000),
             (18, 2000, 1000), (19, 2600, 1000), (20, 2600, 1600), (21, 2000, 1600), (22, 1500, 500),
             (23, 1700, 700), (24, 1800, 900)]
    WAYS = [(100, [1, 2], {"highway": "track", "surface": "gravel", "name": "Alpweg"}),
            (101, [3, 4, 5, 6, 3], {}), (102, [7, 8, 9, 10, 7], {}),
            (103, [11, 12, 13, 11], {"building": "yes", "ruins": "yes"}),
            (104, [14, 15], {"highway": "secondary", "tunnel": "yes"}),
            (105, [16, 17], {"railway": "rail"}),
            (106, [18, 19, 20, 21, 18], {"natural": "water"}),
            (107, [22, 23, 24, 18], {"waterway": "river"})]

    def osm(self):
        nodes = "".join('<node id="{}" version="1" lat="{:.7f}" lon="{:.7f}"/>'.format(
            node, *reversed(location(self.COLUMN, self.ROW, u, v))) for node, u, v in self.NODES)
        tag = lambda tags: "".join(f'<tag k="{key}" v="{value}"/>' for key, value in tags.items())
        ways = "".join(f'<way id="{way}" version="1">' + "".join(f'<nd ref="{node}"/>' for node in refs) + tag(tags)
                       + "</way>" for way, refs, tags in self.WAYS)
        relation = ('<relation id="200" version="1"><member type="way" ref="101" role="outer"/>'
                    '<member type="way" ref="102" role="inner"/>' + tag({"type": "multipolygon", "building": "yes"})
                    + "</relation>")
        return f'<osm version="0.6">{nodes}{ways}{relation}</osm>'

    def test_a_small_region_bakes_into_buffered_tiles_at_every_zoom(self):
        from pmtiles.reader import MmapSource, Reader, all_tiles
        from pmtiles.tile import TileType

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "region.osm").write_text(self.osm())
            west, north = location(self.COLUMN, self.ROW, 0, 0)
            east, south = location(self.COLUMN, self.ROW, 8192 * 2, 8192)
            kinds = features.validate(json.loads(features.TABLE.read_text()))
            features.bake(kinds, root / "region.osm", [west, south, east, north], root / "f.pmtiles", "© OSM")
            with (root / "f.pmtiles").open("rb") as stream:
                source = MmapSource(stream)
                reader = Reader(source)
                self.assertEqual(reader.header()["tile_type"], TileType.UNKNOWN)
                self.assertEqual(reader.metadata()["features_format"], features.FORMAT)
                tiles = {key: features.decode(gzip.decompress(data)) for key, data in all_tiles(source)}
        left, right = tiles[(14, self.COLUMN, self.ROW)], tiles[(14, self.COLUMN + 1, self.ROW)]
        kinds = lambda tile: sorted(feature["kind"] for feature in tile)
        self.assertEqual(kinds(left), ["building", "lake", "river", "road", "ruin", "tunnel"])
        self.assertEqual(kinds(right), ["road"])
        self.assertEqual({key[0] for key in tiles}, set(range(8, 15)))
        buffer = features.pad(14, self.ROW)
        road = next(feature for feature in left if feature["kind"] == "road")
        self.assertEqual(road["tags"], {"highway": "track", "surface": "gravel"})
        self.assertEqual(road["parts"][0]["xy"], [[7000, 4000], [8192 + round(buffer), 4000]])
        self.assertEqual(right[0]["parts"][0]["xy"], [[-round(buffer), 4000], [9500 - 8192, 4000]])
        ground = features.EQUATOR_M * math.cos(math.radians(north)) / (1 << 14) / features.EXTENT
        self.assertGreater(buffer * ground, features.BUFFER_M - 1e-9)
        building = next(feature for feature in left if feature["kind"] == "building")
        self.assertEqual([part["outer"] for part in building["parts"]], [True, False])
        self.assertEqual(len(building["parts"][1]["xy"]), 4)
        # Above zoom 14 a tile holds the water and the main roads only, and the river still ends on
        # the shore of the lake.
        overview = next(tile for (zoom, _, _), tile in tiles.items() if zoom == 11)
        self.assertEqual(kinds(overview), ["lake", "river"])
        lake, river = sorted(overview, key=lambda feature: feature["kind"])
        self.assertIn(river["parts"][0]["xy"][-1], lake["parts"][0]["xy"])


if __name__ == "__main__":
    unittest.main()
