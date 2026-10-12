"""Grid cells carry joined fonts and their route records."""

import json
from pathlib import Path
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


def tile(layer, tags):
    """A vector tile with one layer `layer` and one feature with `tags`."""
    packed = bytes(index for number in range(len(tags)) for index in (number, number))
    return field(3, field(1, layer) + field(2, field(2, packed)) + b"".join(field(3, key.encode()) for key in tags)
                 + b"".join(field(4, field(1, value.encode())) for value in tags.values()))


def archives(source, name, kind="", ref="", poi=""):
    """Write one-tile basemap, places and overlay archives with one feature each, and return them."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write
    tiles = {"basemap": tile(b"places", {"name": name, "kind": kind}), "places": tile(b"pois", {"name": poi}),
             "overlays": tile(b"cycling", {"ref": ref})}
    (source / "maps").mkdir()
    for archive, data in tiles.items():
        with write(source / f"maps/{archive}.pmtiles") as writer:
            writer.write_tile(zxy_to_tileid(0, 0, 0), data)
            writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.NONE,
                             "min_lon_e7": -1800000000, "min_lat_e7": -850000000, "max_lon_e7": 1800000000,
                             "max_lat_e7": 850000000, "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}, {})
    return [source / f"maps/{archive}.pmtiles" for archive in tiles]


class OfflineFonts(unittest.TestCase):
    def test_joined_stack_keeps_only_the_ranges_of_label_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            # `kind` is no label field, so its Cyrillic value adds no range.
            maps = archives(source, "Zähringen–Nord", kind="город", ref="Westweg →")
            files = {}
            for name, data in {"0-255": b"A", "256-511": b"B", "1024-1279": b"C", "8192-8447": b"D", "8448-8703": b"E"}.items():
                path = source / f"maps/assets/fonts/Noto Sans Regular/{name}.pbf"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                files[path.relative_to(source).as_posix()] = {}
            files["maps/assets/fonts/OFL.txt"] = {}
            ranges = blocks.glyph_ranges(blocks.archive_texts(maps))
            joined = blocks.offline_fonts(ranges, source, files, source / "work")
            path, names = next(iter(joined.items()))
            self.assertEqual(len(joined), 1)
            self.assertEqual(path.read_bytes(), b"ADE")
            self.assertEqual(sorted(names), sorted(name for name in files if name.endswith(".pbf")))

    def test_ranges_cover_uppercase_and_arabic_presentation_forms(self):
        with tempfile.TemporaryDirectory() as temporary:
            # µ and ÿ upper-case to U+039C and U+0178; Persian letters shape into U+FB50-U+FEFF.
            maps = archives(Path(temporary), "µÿ", poi="چای")
            self.assertEqual(blocks.glyph_ranges(blocks.archive_texts(maps)), {0, 1, 3, 6, 251, 252, 253, 254})


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


if __name__ == "__main__":
    unittest.main()
