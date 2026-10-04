"""Grid cells carry joined fonts, their route records and complete overlay features."""

import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_blocks as blocks, planner_runtime as runtime


def varint(value):
    data = b""
    while value >= 0x80:
        data += bytes([value & 0x7F | 0x80]); value >>= 7
    return data + bytes([value])


def field(number, payload):
    return varint(number << 3 | 2) + varint(len(payload)) + payload


def release(source, name, kind="", ref=""):
    """Write a one-tile basemap with one feature and an overlay database with one route."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write
    layer = (field(1, b"places") + field(2, field(2, bytes([0, 0, 1, 1]))) + field(3, b"name") + field(3, b"kind")
             + field(4, field(1, name.encode())) + field(4, field(1, kind.encode())))
    (source / "maps").mkdir()
    with write(source / "maps/basemap.pmtiles") as writer:
        writer.write_tile(zxy_to_tileid(0, 0, 0), field(3, layer))
        writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.NONE,
                         "min_lon_e7": -1800000000, "min_lat_e7": -850000000, "max_lon_e7": 1800000000,
                         "max_lat_e7": 850000000, "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}, {})
    (source / "routing").mkdir()
    db = sqlite3.connect(source / "routing/overlays.sqlite")
    db.executescript("CREATE TABLE attributes(id INTEGER PRIMARY KEY, properties TEXT NOT NULL);"
                     "CREATE TABLE routes(id INTEGER PRIMARY KEY, properties TEXT NOT NULL);")
    db.execute("INSERT INTO routes VALUES (1, ?)", (json.dumps({"ref": ref}),))
    db.commit(); db.close()


class OfflineFonts(unittest.TestCase):
    def test_joined_stack_keeps_only_the_ranges_of_label_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            # `kind` is no label field, so its Cyrillic value adds no range.
            release(source, "Zähringen–Nord", kind="город", ref="Westweg →")
            files = {}
            for name, data in {"0-255": b"A", "256-511": b"B", "1024-1279": b"C", "8192-8447": b"D", "8448-8703": b"E"}.items():
                path = source / f"maps/assets/fonts/Noto Sans Regular/{name}.pbf"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                files[path.relative_to(source).as_posix()] = {}
            files["maps/assets/fonts/OFL.txt"] = {}
            joined = blocks.offline_fonts(source, {"files": files}, source / "work")
            path, names = next(iter(joined.items()))
            self.assertEqual(len(joined), 1)
            self.assertEqual(path.read_bytes(), b"ADE")
            self.assertEqual(sorted(names), sorted(name for name in files if name.endswith(".pbf")))

    def test_ranges_cover_uppercase_and_arabic_presentation_forms(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            # µ and ÿ upper-case to U+039C and U+0178; Persian letters shape into U+FB50-U+FEFF.
            release(source, "µÿ چای")
            self.assertEqual(blocks.glyph_ranges(source), {0, 1, 3, 6, 251, 252, 253, 254})


class RouteTiles(unittest.TestCase):
    def test_each_grid_cell_lists_the_records_that_touch_it_in_id_order(self):
        stage = lambda id, cells: {"id": id, "kind": "hiking", "name": f"Stage {id}", "cells": cells,
                                   "line_udeg": [8000000, 47900000, 1000, 0], "via": [], "parent": 10, "stage": id - 20}
        long_route = {"id": 10, "kind": "hiking", "name": "Long", "cells": ["9-1-1", "9-1-2", "9-9-9"],
                      "stages": [21, 22], "start_udeg": [8000000, 47900000]}
        catalog = {"format": 1, "routes": [long_route, stage(21, ["9-1-1"]), stage(22, ["9-1-2", "9-9-9"]),
                                           {**stage(5, ["9-1-2"]), "loop": True}]}
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "region.json"
            path.write_text(json.dumps(catalog))
            tiles = blocks.route_tiles(path, ["9-1-1", "9-1-2", "9-2-1"])
        ids = {name: [record["id"] for record in tile["routes"]] for name, tile in tiles.items()}
        self.assertEqual(ids, {"9-1-1": [10, 21], "9-1-2": [5, 10, 22], "9-2-1": []})
        self.assertEqual(tiles["9-1-2"]["routes"][1], long_route)
        self.assertEqual({tile["format"] for tile in tiles.values()}, {1})


class OverlayShard(unittest.TestCase):
    def test_cell_retains_whole_crossing_features_and_properties(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = (Path(temporary) / name for name in ("source.sqlite", "target.sqlite"))
            with sqlite3.connect(source) as db:
                db.executescript("""PRAGMA user_version=2;
                    CREATE TABLE metadata(package TEXT,coverage TEXT);
                    INSERT INTO metadata VALUES('original','[-2,-2,4,4]');
                    CREATE TABLE geometries(id INTEGER PRIMARY KEY,way INTEGER,points INTEGER,coordinates BLOB);
                    CREATE TABLE attributes(id INTEGER PRIMARY KEY,properties TEXT);
                    CREATE TABLE routes(id INTEGER PRIMARY KEY,properties TEXT);
                    CREATE TABLE features(id INTEGER PRIMARY KEY,kind TEXT,geometry INTEGER,attributes INTEGER);
                    CREATE VIRTUAL TABLE bounds USING rtree(id,west,east,south,north,facet_min,facet_max);""")
                for identity, west, east in [(1, -1, 2), (2, 2, 3), (3, 1, 2)]:
                    db.execute("INSERT INTO geometries VALUES(?,?,?,?)", (identity, identity * 100, 2, bytes([identity, 0, 255])))
                    db.execute("INSERT INTO routes VALUES(?,?)", (identity, json.dumps({"id": identity, "name": "A route", "website": "https://example.com"})))
                    db.execute("INSERT INTO attributes VALUES(?,?)", (identity, json.dumps({"routes": [identity]})))
                    db.execute("INSERT INTO features VALUES(?,?,?,?)", (identity, "cycling", identity, identity))
                    db.execute("INSERT INTO bounds VALUES(?,?,?,?,?,?,?)", (identity, west, east, 0, 1, 8, 8))
            original = runtime.digest(source)
            blocks.overlay_shard(source, target, [0, 0, 1, 1], "new")
            self.assertEqual(runtime.digest(source), original)
            with sqlite3.connect(source) as before, sqlite3.connect(target) as after:
                self.assertEqual(after.execute('PRAGMA user_version').fetchone(), (2,))
                for name in ('features', 'bounds', 'geometries', 'attributes', 'routes'):
                    self.assertEqual(after.execute(f"SELECT * FROM {name} ORDER BY id").fetchall(),
                                     before.execute(f"SELECT * FROM {name} WHERE id IN (1,3) ORDER BY id").fetchall())
                package, coverage = after.execute("SELECT * FROM metadata").fetchone()
                self.assertEqual(package, "new")
                self.assertEqual(json.loads(coverage), [0, 0, 1, 1])


if __name__ == "__main__":
    unittest.main()
