"""Offline selections carry one font file per stack with the glyph ranges the region's labels use."""

import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_blocks as blocks


def varint(value):
    data = b""
    while value >= 0x80:
        data += bytes([value & 0x7F | 0x80]); value >>= 7
    return data + bytes([value])


def field(number, payload):
    return varint(number << 3 | 2) + varint(len(payload)) + payload


class OfflineFonts(unittest.TestCase):
    def test_joined_stack_keeps_only_the_ranges_of_label_text(self):
        from pmtiles.tile import Compression, TileType, zxy_to_tileid
        from pmtiles.writer import write
        # `kind` is no label field, so its Cyrillic value adds no range.
        layer = (field(1, b"places") + field(2, field(2, bytes([0, 0, 1, 1]))) + field(3, b"name") + field(3, b"kind")
                 + field(4, field(1, "Zähringen–Nord".encode())) + field(4, field(1, "город".encode())))
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
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
            db.execute("INSERT INTO routes VALUES (1, ?)", (json.dumps({"ref": "Westweg →"}),))
            db.commit(); db.close()
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


if __name__ == "__main__":
    unittest.main()
