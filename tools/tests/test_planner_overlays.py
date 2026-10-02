"""The overlay archive draws each zoom's routes and the access rules, with their details in the tiles."""

import gzip
import json
import math
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_mvt as mvt
from tools import planner_overlays as overlays


def blob(points):
    """The overlay index geometry: Postcard `Vec<[i32;2]>` of microdegree differences."""
    values, previous = [], (0, 0)
    for point in points:
        values += [round(point[0] * 1e6) - previous[0], round(point[1] * 1e6) - previous[1]]
        previous = (round(point[0] * 1e6), round(point[1] * 1e6))
    return mvt.varint(len(points)) + b"".join(mvt.varint(mvt.zigzag(v)) for v in values)


def layers(tile):
    """Each layer of a tile as {feature ID: properties}."""
    result = {}
    for _, layer in mvt.fields(gzip.decompress(tile)):
        parts = list(mvt.fields(layer))
        keys = [v.decode() for f, v in parts if f == 3]
        values = [next(v.decode() if f == 1 else v for f, v in mvt.fields(value)) for f, value in parts if f == 4]
        name = next(v.decode() for f, v in parts if f == 1)
        for f, feature in parts:
            if f != 2: continue
            fields = dict(mvt.fields(feature))
            tags = mvt.packed(fields.get(2, b""))
            result.setdefault(name, {})[fields[1]] = {keys[k]: values[v] for k, v in zip(tags[::2], tags[1::2])}
    return result


class OverlayArchive(unittest.TestCase):
    def test_tiles_carry_the_routes_of_each_zoom_and_the_access_details(self):
        from pmtiles.reader import MmapSource, Reader
        from pmtiles.tile import zxy_to_tileid
        route = {"kind": "cycling", "symbol": "", "symbol_text": ""}
        routes = {1: {**route, "id": 1, "name": "EuroVelo 6", "network": "icn", "rank": 4, "ref": "EV6", "website": "https://ev.example"},
                  2: {**route, "id": 2, "name": "", "network": "lcn", "rank": 1, "ref": "L1", "website": None}}
        access = {"conditional": 0, "cycling_status": "push", "walking_status": "", "name": "Steg", "ref": "",
                  "riding": [False, False], "walking": [True, True], "pushing": [True, True], "tags": {"bicycle": "dismount"}}
        with tempfile.TemporaryDirectory() as directory:
            index, target = Path(directory) / "overlays.sqlite", Path(directory) / "overlays.pmtiles"
            with sqlite3.connect(index) as db:
                db.executescript("""CREATE TABLE metadata(package TEXT, coverage TEXT); CREATE TABLE routes(id INTEGER PRIMARY KEY, properties TEXT);
                    CREATE TABLE geometries(id INTEGER PRIMARY KEY, way INTEGER, points INTEGER, coordinates BLOB);
                    CREATE TABLE attributes(id INTEGER PRIMARY KEY, properties TEXT);
                    CREATE TABLE features(id INTEGER PRIMARY KEY, kind TEXT, cycling_minzoom REAL, walking_minzoom REAL, geometry INTEGER, attributes INTEGER);""")
                db.execute("INSERT INTO metadata VALUES (?, ?)", ("a" * 64, "[7.8,47.9,7.9,48.0]"))
                db.executemany("INSERT INTO routes VALUES (?, ?)", [(k, json.dumps(v)) for k, v in routes.items()])
                # Two ways of both routes meet at one node; a third way carries only the local route.
                db.executemany("INSERT INTO geometries VALUES (?, ?, 2, ?)", [
                    (1, 10, blob([(7.85, 47.95), (7.852, 47.95)])), (2, 11, blob([(7.852, 47.95), (7.854, 47.951)])),
                    (3, 12, blob([(7.86, 47.96), (7.861, 47.96)])), (4, 13, blob([(7.854, 47.951), (7.856, 47.952)]))])
                db.executemany("INSERT INTO attributes VALUES (?, ?)", [(1, json.dumps({"rank": 4, "ref": "EV6", "routes": [1, 2]})),
                    (2, json.dumps(access)), (3, json.dumps({"rank": 1, "ref": "L1", "routes": [2]}))])
                db.executemany("INSERT INTO features(kind, cycling_minzoom, walking_minzoom, geometry, attributes) VALUES (?, ?, ?, ?, ?)", [
                    ("cycling", 6, 6, 1, 1), ("cycling", 6, 6, 2, 1), ("access", 15, None, 3, 2), ("cycling", 11, 11, 4, 3)])
            overlays.derive(index, target)
            with target.open("rb") as stream:
                reader = Reader(MmapSource(stream))
                metadata = reader.metadata()
                tile = lambda z: layers(reader.get(z, *(int(v * (1 << z)) for v in overlays.coordinates(blob([(7.85, 47.95)])))))
                low, high = tile(6), tile(14)
        self.assertEqual(metadata["routing_package"], "a" * 64)
        # One line draws both ways at zoom 6, and it names only the route that zoom draws.
        self.assertEqual(low["cycling"], {10: {"rank": 4, "ref": "EV6", "routes": "[1]"}})
        self.assertEqual(low["routes"], {1: {"kind": "cycling", "name": "EuroVelo 6", "network": "icn", "rank": 4, "ref": "EV6",
                                             "website": "https://ev.example"}})
        self.assertEqual(high["cycling"][10]["routes"], "[1, 2]")
        self.assertEqual(high["routes"][2], {"kind": "cycling", "network": "lcn", "rank": 1, "ref": "L1"})
        self.assertEqual(high["access"][12], {**access, "riding": "[false,false]", "walking": "[true,true]", "pushing": "[true,true]",
                                              "tags": '{"bicycle":"dismount"}', "cycling_minzoom": 15})
        self.assertEqual(set(low), {"cycling", "routes"})


if __name__ == "__main__":
    unittest.main()
