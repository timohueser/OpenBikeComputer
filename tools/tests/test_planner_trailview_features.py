"""The kind table, the feature tile codec and the bake of a small OSM file."""

import gzip
import io
import json
import math
import os
from pathlib import Path
import shutil
import tempfile
import unittest

import numpy as np

from tools import planner_trailview_features as features, region_osm
from tools.trailview import roads, structures

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
    """Longitude and latitude of the point (u, v) in tile units of the deepest tile (column, row)."""
    x, y = (column + u / features.EXTENT) / (1 << features.MAX_ZOOM), (row + v / features.EXTENT) / (1 << features.MAX_ZOOM)
    return x * 360 - 180, math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y))))


class Generalization(unittest.TestCase):
    def test_an_island_near_a_curved_shore_does_not_cross_it_at_any_zoom(self):
        # A lake whose shore curves 45 m over 2 km, with a 200 m island 8 m off the shore.
        metres = lambda x, y: features.world(8.2 + x / 76_220, 46.8 + y / 111_320)
        shore = [(x, -45 * (1 - ((x - 1000) / 1000) ** 2)) for x in range(0, 2001, 50)] + [(2000, 800), (0, 800)]
        island = [(900, -36), (1100, -36), (1100, 164), (900, 164)]
        lake = [(True, [metres(*point) for point in shore], None), (False, [metres(*point) for point in island], None)]
        side = lambda p, q, r: (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
        cross = lambda a, b, c, d: side(a, b, c) * side(a, b, d) < 0 and side(c, d, a) * side(c, d, b) < 0
        for zoom in range(features.MIN_ZOOM, features.MAX_ZOOM):
            tolerance = features.TOLERANCE / ((1 << zoom) * features.EXTENT)
            rings = [points for _, points, _ in features.simplified("area", lake, [[], []], tolerance)]
            edges = [edge for ring in rings for edge in zip(ring, ring[1:] + ring[:1])]
            crossing = [(e, f) for n, e in enumerate(edges) for f in edges[n + 1:] if len({*e, *f}) == 4 and cross(*e, *f)]
            with self.subTest(zoom=zoom):
                self.assertLess(len(rings[0]), len(shore))
                self.assertEqual((len(rings[1]), crossing), (4, []))


@unittest.skipUnless(shutil.which(os.environ.get("OBC_OSMIUM", "osmium")) or os.environ.get("CI"),
                     "Prepared Osmium is unavailable")
class Bridges(unittest.TestCase):
    # Metres east and north of a point at about 46.7° N, in world units.
    Y = 0.3218
    M = 1 / structures.metres(Y)

    def at(self, east, north=0.0):
        return (0.53 + east * self.M, self.Y - north * self.M)

    class Flat:
        def heights(self, x, y):
            return np.full(np.shape(x), 500.0)

    def test_a_tunnel_under_a_mountain_does_not_float_and_over_a_valley_it_does(self):
        class Mountain:
            def heights(_, x, y):
                return np.where(np.abs(np.asarray(x) - 0.53) < 2995 * self.M, 2500.0, 1000.0)
        points = [self.at(-3000), self.at(-1000), self.at(0), self.at(3000)]
        z = [1000.0] * len(points)
        self.assertFalse(structures.floating(points, z, Mountain()))
        self.assertTrue(structures.floating(points, z, self.Flat()))

    def test_a_pier_steps_out_of_a_narrow_channel_and_a_high_footbridge_hangs(self):
        ways = [("road", {}, [self.at(0), self.at(80)], [500.0, 500.0]), ("path", {}, [self.at(0, 50), self.at(120, 50)], [600.0, 600.0])]
        added, piers, metrics = structures.bridges(ways, [(5.0, [self.at(40, -50), self.at(40, 100)])], [], self.Flat())
        (east, _), _, _ = piers[0]
        self.assertEqual(len(piers), 1)
        self.assertGreater(abs((east - 0.53) / self.M - 40), 5 + structures.BANK_M)
        self.assertEqual(added[1]["bridge:structure"], "simple-suspension")
        self.assertEqual(metrics["cable_bridges"], 1)

    def test_a_pier_stands_beside_the_road_under_its_bridge(self):
        ways = [("road", {}, [self.at(0), self.at(80)], [510.0, 510.0])]
        under = [(structures.ROAD_HALF_M, [self.at(40, -50), self.at(40, 50)])]
        _, piers, _ = structures.bridges(ways, [], [], self.Flat(), under)
        (east, _), _, _ = piers[0]
        self.assertGreater(abs((east - 0.53) / self.M - 40), structures.ROAD_HALF_M + structures.BANK_M)


class Bake(unittest.TestCase):
    # A zoom-13 tile in Engelberg. Each node is (id, u, v) in its tile units.
    COLUMN, ROW = 4286, 2889
    NODES = [(1, 7000, 4000), (2, 17500, 4000), (3, 3000, 3000), (4, 3400, 3000), (5, 3400, 3400), (6, 3000, 3400),
             (7, 3100, 3100), (8, 3100, 3200), (9, 3200, 3200), (10, 3200, 3100), (11, 5000, 5000), (12, 5100, 5000),
             (13, 5100, 5100), (14, 1000, 6000), (15, 1500, 6000), (16, 1000, 7000), (17, 2000, 7000),
             (18, 2000, 1000), (19, 2600, 1000), (20, 2600, 1600), (21, 2000, 1600), (22, 1500, 500),
             (23, 1700, 700), (24, 1800, 900), (25, 6000, 1000), (26, 6300, 1000), (27, 6300, 1100),
             (28, 7000, 1000), (29, 7300, 1000), (30, 7300, 1100), (31, 6000, 2000), (32, 6200, 2000),
             (33, 6000, 2500), (34, 6400, 2500), (35, 6000, 3000), (36, 2900, 2900), (37, 3500, 2900),
             (38, 3500, 3500), (39, 2900, 3500)]
    NODE_TAGS = {35: {"waterway": "weir"}}
    WAYS = [(100, [1, 2], {"highway": "track", "surface": "gravel", "name": "Alpweg"}),
            (101, [3, 4, 5, 6, 3], {}), (102, [7, 8, 9, 10, 7], {}),
            (103, [11, 12, 13, 11], {"building": "yes", "ruins": "yes"}),
            (104, [14, 15], {"highway": "secondary", "tunnel": "yes"}),
            (105, [16, 17], {"railway": "rail"}),
            (106, [18, 19, 20, 21, 18], {"natural": "water"}),
            (107, [22, 23, 24, 18], {"waterway": "river"}),
            # Dams: a closed way without and with area=yes, and a crest.
            (108, [25, 26, 27, 25], {"waterway": "dam"}),
            (109, [28, 29, 30, 28], {"waterway": "dam", "area": "yes"}),
            (110, [31, 32], {"waterway": "dam"}),
            (111, [33, 34], {"highway": "construction", "tunnel": "yes"}),
            (112, [36, 37, 38, 39, 36], {"landuse": "farmyard"})]

    def osm(self):
        tag = lambda tags: "".join(f'<tag k="{key}" v="{value}"/>' for key, value in tags.items())
        nodes = "".join('<node id="{}" version="1" lat="{:.7f}" lon="{:.7f}">'.format(
            node, *reversed(location(self.COLUMN, self.ROW, u, v))) + tag(self.NODE_TAGS.get(node, {})) + "</node>"
            for node, u, v in self.NODES)
        ways = "".join(f'<way id="{way}" version="1">' + "".join(f'<nd ref="{node}"/>' for node in refs) + tag(tags)
                       + "</way>" for way, refs, tags in self.WAYS)
        relation = ('<relation id="200" version="1"><member type="way" ref="101" role="outer"/>'
                    '<member type="way" ref="102" role="inner"/>' + tag({"type": "multipolygon", "building": "yes"})
                    + "</relation>")
        return f'<osm version="0.6">{nodes}{ways}{relation}</osm>'

    def terrain(self, root):
        """The grid of `planner/terrain` with a zoom-13 tile under each of the two feature tiles: a plane
        that rises 5 cm per pixel to the east."""
        from PIL import Image
        from pmtiles.tile import Compression, TileType, zxy_to_tileid
        from pmtiles.writer import Writer

        x, y = self.COLUMN, self.ROW
        (root / "objects").mkdir()
        with (root / "objects/archive").open("wb") as stream:
            writer = Writer(stream)
            for east in (0, 1):
                heights = 1000 + 0.05 * (512 * east + np.arange(512, dtype=np.float64))[None, :].repeat(512, 0)
                code = np.round((heights + 32768) * 256).astype(np.int64)
                rgba = np.stack([code >> 16, (code >> 8) & 255, code & 255, np.full(code.shape, 255)], -1).astype(np.uint8)
                data = io.BytesIO()
                Image.fromarray(rgba, "RGBA").save(data, format="WEBP", lossless=True, exact=True)
                writer.write_tile(zxy_to_tileid(13, x + east, y), data.getvalue())
            writer.finalize({"tile_type": TileType.WEBP, "tile_compression": Compression.NONE}, {})
        name = f"maps/tiles/terrain/13/11-{x >> 2}-{y >> 2}.pmtiles"
        (root / "index.json").write_text(json.dumps({"files": {name: {"sha256": "archive"}}}))
        files = {"index.json": str(root / "index.json"), "objects/archive": str(root / "objects/archive")}
        # The height at u tile units east in the tile COLUMN: its pixel of zoom 13 is u / 32.
        return files, lambda u: 1000 + 0.05 * (u / 32 - 0.5)

    def test_a_small_region_bakes_into_buffered_tiles_at_every_zoom(self):
        from pmtiles.reader import MmapSource, Reader, all_tiles
        from pmtiles.tile import TileType

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "region.osm").write_text(self.osm())
            # The step reads a store object, which has no file extension.
            region_osm.run(region_osm.probe()[0], "cat", root / "region.osm", "-f", "pbf", "-o", root / "object")
            west, north = location(self.COLUMN, self.ROW, 0, 0)
            east, south = location(self.COLUMN, self.ROW, 2 * features.EXTENT, features.EXTENT)
            bounds = [west, south, east, north]
            (root / "output").mkdir()
            kinds = json.loads(features.TABLE.read_text())
            grid, height = self.terrain(root)
            classes = json.loads(roads.TABLE.read_text())
            features.step({"options": {"bounds": bounds, "kinds": kinds, "roads": classes, "attribution": "© OSM"},
                           "layers": {"planner/osm": {"osm.pbf": str(root / "object")}, "planner/terrain/grid": grid},
                           "output": str(root / "output"), "metrics": str(root / "metrics.json")})
            with (root / "output/trailview-features.pmtiles").open("rb") as stream:
                source = MmapSource(stream)
                reader = Reader(source)
                self.assertEqual(reader.header()["tile_type"], TileType.UNKNOWN)
                self.assertEqual(reader.metadata()["features_format"], features.FORMAT)
                self.assertEqual(reader.metadata()["bounds"], bounds)
                tiles = {key: features.decode(gzip.decompress(data)) for key, data in all_tiles(source)}
        left, right = tiles[(features.MAX_ZOOM, self.COLUMN, self.ROW)], tiles[(features.MAX_ZOOM, self.COLUMN + 1, self.ROW)]
        kinds = lambda tile: sorted(feature["kind"] for feature in tile)
        self.assertEqual(kinds(left), ["building", "dam", "dam-area", "dam-area", "dam-point", "lake", "main-road", "railway",
                                       "river", "road", "ruin"])
        self.assertEqual(kinds(right), ["road"])
        # The building in the farmyard keeps its landuse; the farmyard is no feature.
        self.assertEqual(next(feature for feature in left if feature["kind"] == "building")["tags"], {"building": "yes", "landuse": "farmyard"})
        self.assertEqual({key[0] for key in tiles}, set(range(features.MIN_ZOOM, features.MAX_ZOOM + 1)))
        dam = next(feature for feature in left if feature["kind"] == "dam")
        self.assertEqual(len(dam["parts"][0]["xy"]), 2, "a closed dam way is an area, not a ring line")
        # A tunnel with no terrain over it runs on the ground, as a road.
        tunnel = next(feature for feature in left if feature["kind"] == "main-road")
        self.assertEqual(tunnel["tags"], {"highway": "secondary"})
        buffer = features.pad(features.MAX_ZOOM, self.ROW)
        road = next(feature for feature in left if feature["kind"] == "road")
        self.assertEqual(road["tags"], {"highway": "track", "surface": "gravel"})
        # A cut keeps the segment whole, so both tiles hold the same one, and its heights follow the terrain.
        self.assertEqual(road["parts"][0]["xy"], [[7000, 4000], [17500, 4000]])
        self.assertEqual(right[0]["parts"][0]["xy"], [[7000 - features.EXTENT, 4000], [17500 - features.EXTENT, 4000]])
        for tile in (road, right[0]):
            self.assertEqual(tile["parts"][0]["z"], [round(height(7000), 2), round(height(17500), 2)])
        # A tunnel that meets no road takes the terrain at its ends: nothing else gives it heights.
        for z, u in zip(tunnel["parts"][0]["z"], (1000, 1500)):
            self.assertAlmostEqual(z, height(u), delta=0.05)
        lake = next(feature for feature in left if feature["kind"] == "lake")
        level = lake["parts"][0]["z"]
        self.assertEqual(len(set(level)), 1, "a lake has one height")
        self.assertLess(height(2000), level[0], "a low quantile of its shore, inside it")
        self.assertLess(level[0], height(2300))
        # The river flows east, up the plane, into the lake: it holds near the lake's level and never rises.
        stream = next(feature for feature in left if feature["kind"] == "river")["parts"][0]["z"]
        self.assertEqual(stream, sorted(stream, reverse=True))
        self.assertLess(abs(stream[-1] - level[0]), 0.05)
        ground = features.EQUATOR_M * math.cos(math.radians(north)) / (1 << features.MAX_ZOOM) / features.EXTENT
        self.assertGreater(buffer * ground, features.BUFFER_M - 1e-9)
        building = next(feature for feature in left if feature["kind"] == "building")
        self.assertEqual([part["outer"] for part in building["parts"]], [True, False])
        self.assertEqual(len(building["parts"][1]["xy"]), 4)
        # Above zoom 13 a tile holds the water and the main roads only, and the river still ends on
        # the shore of the lake.
        overview = next(tile for (zoom, _, _), tile in tiles.items() if zoom == 11)
        self.assertEqual(kinds(overview), ["lake", "main-road", "river"])
        lake, _, river = sorted(overview, key=lambda feature: feature["kind"])
        self.assertIn(river["parts"][0]["xy"][-1], lake["parts"][0]["xy"])

    def test_the_input_holds_what_the_region_can_place_and_nothing_past_it(self):
        west, north = location(self.COLUMN, self.ROW, 0, 0)
        east, south = location(self.COLUMN, self.ROW, 2 * features.EXTENT, features.EXTENT)
        lon, lat = location(self.COLUMN, self.ROW, 1000, 1000)
        # Way 1 is a building near the region, and way 2 one past the near area, which only zoom 13
        # places. Way 3 is a lake there, within one tile of zoom 8, which zoom 8 places; way 4 a lake past
        # that. Way 5 near the region is of no kind.
        ways = [(1, lon, {"building": "yes"}), (2, lon + 0.6, {"building": "yes"}), (3, lon + 0.6, {"natural": "water"}),
                (4, lon + 4.0, {"natural": "water"}), (5, lon + 0.002, {"amenity": "bench"})]
        nodes, points, lines = 0, "", ""
        for way, x, tags in ways:
            refs = []
            for dx, dy in ((0, 0), (0.001, 0), (0.001, 0.001), (0, 0.001)):
                nodes += 1
                refs.append(nodes)
                points += f'<node id="{nodes}" version="1" lat="{lat + dy:.7f}" lon="{x + dx:.7f}"/>'
            tag = "".join(f'<tag k="{key}" v="{value}"/>' for key, value in tags.items())
            lines += f'<way id="{100 + way}" version="1">' + "".join(f'<nd ref="{ref}"/>' for ref in refs + refs[:1]) + tag + "</way>"
        with tempfile.TemporaryDirectory() as temporary:
            root, binary = Path(temporary), region_osm.probe()[0]
            (root / "region.osm").write_text(f'<osm version="0.6">{points}{lines}</osm>')
            region_osm.run(binary, "cat", root / "region.osm", "-o", root / "region.osm.pbf")
            kinds = json.loads(features.TABLE.read_text())
            chosen = features.selected(kinds, root / "region.osm.pbf", binary, features.Tiles([west, south, east, north]), root)
            text = region_osm.run(binary, "cat", chosen, "-f", "opl").decode()
        self.assertEqual([int(line.split()[0][1:]) for line in text.splitlines() if line.startswith("w")], [101, 103])


if __name__ == "__main__":
    unittest.main()
