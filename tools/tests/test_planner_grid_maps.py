"""Map grid steps preserve tile bytes and publish only packed objects."""

import gzip
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

import numpy as np
from PIL import Image
from pmtiles.reader import MmapSource, Reader, all_tiles
from pmtiles.tile import Compression, TileType, zxy_to_tileid
from pmtiles.writer import write

from tools import planner_grid_maps as grid, planner_offline as offline, planner_verify


BOUNDS = [-180, -85, 180, 85]


class MapGrid(unittest.TestCase):
    def request(self, root, source, kind):
        output = root / "output"
        output.mkdir()
        return {"output": str(output), "metrics": str(root / "metrics.json"),
                "layers": {f"planner/{kind}": {source.name: str(source)}},
                "options": {"kind": kind, "bounds": BOUNDS}}

    def files(self, request):
        output = Path(request["output"])
        index = json.loads((output / "index.json").read_bytes())
        for entry in index["files"].values():
            offline.verify(output / "objects" / entry["transport"]["sha256"], entry["transport"])
        self.assertEqual(set(path.name for path in output.iterdir()), {"objects", "index.json"})
        return output, index

    def test_tiles_share_their_zoom_11_parent_and_keep_every_source_byte(self):
        tiles = {(0, 0, 0): b"world", (12, 10, 20): b"town", (14, 41, 81): b"road", (14, 48, 81): b"next"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "basemap.pmtiles"
            with write(archive) as writer:
                for tile, data in sorted(tiles.items(), key=lambda item: zxy_to_tileid(*item[0])):
                    writer.write_tile(zxy_to_tileid(*tile), data)
                writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.NONE,
                                 "min_lon_e7": -1800000000, "min_lat_e7": -850000000,
                                 "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                                 "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0},
                                {"attribution": "test credit", "vector_layers": [{"id": "roads", "fields": {}}]})
            request = self.request(root, archive, "basemap")
            grid.step(request)
            output, index = self.files(request)
            self.assertEqual(index["source"], offline.item(archive))
            parts = {name: entry for name, entry in index["files"].items() if name.endswith(".pmtiles")}
            self.assertEqual(set(parts), {f"maps/tiles/basemap/{name}.pmtiles" for name in ["0-0-0", "11-5-10", "11-6-10"]})
            restored = {}
            for entry in parts.values():
                self.assertEqual(entry["transport"]["encoding"], "identity")
                with (output / "objects" / entry["transport"]["sha256"]).open("rb") as stream:
                    read = MmapSource(stream)
                    restored.update(dict(all_tiles(read)))
                    self.assertEqual(Reader(read).metadata()["attribution"], "test credit")
            self.assertEqual(restored, tiles)
            entry = index["files"]["maps/basemap.json"]
            data = (output / "objects" / entry["transport"]["sha256"]).read_bytes()
            if entry["transport"]["encoding"] == "gzip":
                data = gzip.decompress(data)
            self.assertEqual(json.loads(data), index["metadata"])
            self.assertEqual(index["metadata"]["bounds"], BOUNDS)

    def test_terrain_keeps_its_pixels_in_fewer_bytes_and_zoom_13_in_its_own_archives(self):
        heights = (np.arange(64 * 64).reshape(64, 64) * 37 % 9973).astype(np.int64) + 32768 * 256
        rgba = np.stack([heights >> 16, heights >> 8 & 255, heights & 255, np.full_like(heights, 255)], -1)
        rgba[0, 0] = (128, 0, 0, 0)
        image = io.BytesIO()
        Image.fromarray(rgba.astype(np.uint8), "RGBA").save(image, format="WEBP", lossless=True, method=0, exact=True)
        data = image.getvalue()
        tiles = {(12, 2000, 1400): "11-1000-700", (13, 4001, 2801): "13/11-1000-700"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "terrain.mbtiles"
            with sqlite3.connect(archive) as db:
                db.executescript("CREATE TABLE metadata(name TEXT, value TEXT);"
                                 "CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
                db.executemany("INSERT INTO metadata VALUES (?,?)", [("format", "webp"), ("name", "Terrain"),
                    ("bounds", "-180,-85,180,85"), ("minzoom", "0"), ("maxzoom", "13")])
                db.executemany("INSERT INTO tiles VALUES (?,?,?,?)", [(z, x, (1 << z) - 1 - y, data) for z, x, y in tiles])
            request = self.request(root, archive, "terrain")
            grid.step(request)
            output, index = self.files(request)
            self.assertEqual(set(index["files"]), {"maps/terrain.json", *(f"maps/tiles/terrain/{name}.pmtiles" for name in tiles.values())})
            self.assertEqual((index["metadata"]["maxzoom"], index["metadata"]["trailview_maxzoom"]), (12, 13))
            for tile, name in tiles.items():
                path = output / "objects" / index["files"][f"maps/tiles/terrain/{name}.pmtiles"]["transport"]["sha256"]
                planner_verify.archive(path, "terrain")
                with path.open("rb") as stream:
                    read = MmapSource(stream)
                    (found, encoded), = all_tiles(read)
                    self.assertEqual(Reader(read).header()["tile_compression"], Compression.NONE)
                self.assertEqual(found, tile)
                self.assertLess(len(encoded), len(data))
                self.assertEqual(np.asarray(Image.open(io.BytesIO(encoded)).convert("RGBA")).tolist(), rgba.tolist())
            with self.assertRaisesRegex(ValueError, 'tile format'):
                planner_verify.archive(path, "basemap")

    def test_trailview_archives_verify_as_their_raster_tiles(self):
        def tile(format, **options):
            image = io.BytesIO()
            Image.new("RGBA", (2, 2), (90, 120, 60, 200)).save(image, format=format, **options)
            return image.getvalue()

        webp, avif = tile("WEBP", lossless=True), tile("AVIF")
        for kind, tile_type, data, ok in [("trailview-material", TileType.WEBP, webp, True),
                                          ("trailview-colour-ndvimax", TileType.WEBP, webp, True),
                                          ("trailview-colour-q1", TileType.AVIF, avif, True),
                                          ("trailview-colour-q1", TileType.WEBP, webp, True),
                                          ("trailview-material", TileType.AVIF, avif, False)]:
            with self.subTest(kind=kind, tile_type=tile_type), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "archive.pmtiles"
                with write(path) as writer:
                    writer.write_tile(zxy_to_tileid(9, 0, 0), data)
                    writer.finalize({"tile_type": tile_type, "tile_compression": Compression.NONE, "min_zoom": 9, "max_zoom": 9},
                                    {"trailview_format": 1})
                if ok:
                    planner_verify.archive(path, kind)
                else:
                    with self.assertRaisesRegex(ValueError, "tile format"):
                        planner_verify.archive(path, kind)

    def test_changed_binary_archives_check_the_decoded_consumer_layouts(self):
        for kind, zoom, size, metadata in [('snow', 8, 2 * 2 * 256 * 256, {'seasons': 2}),
                                         ('climate', 8, 183552, {'years': 10}),
                                         ('climate', 9, 252096, {'years': 10})]:
            with self.subTest(kind=kind, zoom=zoom), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / 'archive.pmtiles'
                for length in (0, size - 1, size):
                    with write(path) as writer:
                        writer.write_tile(zxy_to_tileid(zoom, 0, 0), gzip.compress(bytes(length)))
                        writer.finalize({'tile_type': TileType.UNKNOWN, 'tile_compression': Compression.GZIP}, metadata)
                    if length == size:
                        planner_verify.archive(path, kind)
                    else:
                        with self.assertRaisesRegex(ValueError, 'tile size'):
                            planner_verify.archive(path, kind)

    def test_empty_terrain_has_metadata_and_no_tile_archives(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "terrain.mbtiles"
            with sqlite3.connect(archive) as db:
                db.executescript("CREATE TABLE metadata(name TEXT, value TEXT);"
                                 "CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
                db.executemany("INSERT INTO metadata VALUES (?,?)", [("format", "webp"),
                    ("minzoom", "6"), ("maxzoom", "14"), ("attribution", "test terrain credit")])
            request = self.request(root, archive, "terrain")
            grid.step(request)
            _, index = self.files(request)
            self.assertEqual(set(index["files"]), {"maps/terrain.json"})
            self.assertEqual(index["metadata"]["minzoom"], 6)
            self.assertEqual(index["metadata"]["maxzoom"], 14)
            self.assertEqual(index["metadata"]["attribution"], "test terrain credit")
            self.assertEqual(index["source"], offline.item(archive))


if __name__ == "__main__":
    unittest.main()
