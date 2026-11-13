"""Map grid steps preserve tile bytes and publish only packed objects."""

import gzip
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

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

    def source(self, root, kind, tiles, tile_type=TileType.MVT):
        archive = root / f"{kind}.pmtiles"
        with write(archive) as writer:
            for tile, data in sorted(tiles.items(), key=lambda item: zxy_to_tileid(*item[0])):
                writer.write_tile(zxy_to_tileid(*tile), data)
            writer.finalize({"tile_type": tile_type, "tile_compression": Compression.NONE,
                             "min_lon_e7": -1800000000, "min_lat_e7": -850000000,
                             "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                             "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0},
                            {"attribution": "test credit", "vector_layers": [{"id": "roads", "fields": {}}]})
        return archive

    def packs(self, output, index):
        """The tiles of each pack, by pack name."""
        packs = {}
        for name, entry in index["files"].items():
            if not name.endswith(".pmtiles"): continue
            self.assertEqual(entry["transport"]["encoding"], "identity")
            with (output / "objects" / entry["transport"]["sha256"]).open("rb") as stream:
                read = MmapSource(stream)
                packs[name] = dict(all_tiles(read))
                self.assertEqual(Reader(read).metadata()["attribution"], "test credit")
        return packs

    def test_tiles_share_the_pack_of_their_base_square_and_keep_every_source_byte(self):
        tiles = {(0, 0, 0): b"world", (8, 5, 10): b"region", (12, 10, 20): b"town", (14, 41, 81): b"road", (14, 48, 81): b"next"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = self.source(root, "basemap", tiles)
            request = self.request(root, archive, "basemap")
            grid.step(request)
            output, index = self.files(request)
            self.assertEqual(index["source"], offline.item(archive))
            self.assertNotIn("map_zoom", index)
            self.assertEqual(self.packs(output, index), {
                "maps/tiles/basemap/0-0-0.pmtiles": {(0, 0, 0): b"world"},
                "maps/tiles/basemap/7-2-5.pmtiles": {(8, 5, 10): b"region"},
                "maps/tiles/basemap/9-1-2.pmtiles": {(12, 10, 20): b"town", (14, 41, 81): b"road", (14, 48, 81): b"next"}})
            entry = index["files"]["maps/basemap.json"]
            data = (output / "objects" / entry["transport"]["sha256"]).read_bytes()
            if entry["transport"]["encoding"] == "gzip":
                data = gzip.decompress(data)
            self.assertEqual(json.loads(data), index["metadata"])
            self.assertEqual(index["metadata"]["bounds"], BOUNDS)

    def test_a_pack_over_the_limit_keeps_its_own_tile_and_its_children_take_the_deeper_tiles(self):
        tiles = {(9, 1, 2): b"a" * 1000, (10, 2, 4): b"b" * 1000, (10, 3, 5): b"c" * 1000, (11, 6, 10): b"d" * 1000}
        with tempfile.TemporaryDirectory() as temporary, patch.object(grid, "PACK_BYTES", 3000):
            root = Path(temporary)
            request = self.request(root, self.source(root, "basemap", tiles), "basemap")
            grid.step(request)
            self.assertEqual(self.packs(*self.files(request)), {
                "maps/tiles/basemap/9-1-2.pmtiles": {(9, 1, 2): b"a" * 1000},
                "maps/tiles/basemap/10-2-4.pmtiles": {(10, 2, 4): b"b" * 1000},
                "maps/tiles/basemap/10-3-5.pmtiles": {(10, 3, 5): b"c" * 1000, (11, 6, 10): b"d" * 1000}})

    def test_a_zoom_13_terrain_pack_over_the_limit_has_no_parent_file_and_one_large_tile_fails(self):
        def noise(seed):
            pixels = np.random.default_rng(seed).integers(0, 256, (32, 32, 4), dtype=np.uint8)
            pixels[..., 3] = 255
            image = io.BytesIO()
            Image.fromarray(pixels, "RGBA").save(image, format="WEBP", lossless=True)
            return image.getvalue()

        tiles = {(13, 4001, 2801): noise(1), (13, 4009, 2801): noise(2)}
        for limit, packs in [(len(tiles[13, 4001, 2801]) * 3 // 2, {"13/10-500-350", "13/10-501-350"}), (1000, None)]:
            with self.subTest(limit=limit), tempfile.TemporaryDirectory() as temporary, patch.object(grid, "PACK_BYTES", limit):
                root = Path(temporary)
                request = self.request(root, self.source(root, "terrain", tiles, TileType.WEBP), "terrain")
                if packs is None:
                    with self.assertRaisesRegex(ValueError, "one tile"):
                        grid.step(request)
                    continue
                grid.step(request)
                _, index = self.files(request)
                self.assertEqual(set(index["files"]), {"maps/terrain.json", *(f"maps/tiles/terrain/{name}.pmtiles" for name in packs)})

    def test_terrain_keeps_its_pixels_in_fewer_bytes_and_zoom_13_in_its_own_archives(self):
        heights = (np.arange(64 * 64).reshape(64, 64) * 37 % 9973).astype(np.int64) + 32768 * 256
        rgba = np.stack([heights >> 16, heights >> 8 & 255, heights & 255, np.full_like(heights, 255)], -1)
        rgba[0, 0] = (128, 0, 0, 0)
        image = io.BytesIO()
        Image.fromarray(rgba.astype(np.uint8), "RGBA").save(image, format="WEBP", lossless=True, method=0, exact=True)
        data = image.getvalue()
        tiles = {(12, 2000, 1400): "9-250-175", (13, 4001, 2801): "13/9-250-175"}
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
        def tile(format, size=512, **options):
            image = io.BytesIO()
            Image.new("RGBA", (size, size), (90, 120, 60, 200)).save(image, format=format, **options)
            return image.getvalue()

        webp, avif, ndvi = tile("WEBP", lossless=True), tile("AVIF"), tile("WEBP", 256, lossless=True)
        for kind, tile_type, data, ok in [("trailview-ground", TileType.WEBP, webp, True),
                                          ("trailview-colour-q1", TileType.UNKNOWN, ndvi + avif, True),
                                          ("trailview-colour-q1", TileType.UNKNOWN, ndvi + webp, True),
                                          ("trailview-colour-q1", TileType.UNKNOWN, webp + ndvi, False),
                                          ("snow", TileType.WEBP, webp, True),
                                          ("snow", TileType.WEBP, ndvi, False),
                                          ("snow-levels", TileType.WEBP, webp * 4, True),
                                          ("snow-levels", TileType.WEBP, webp * 3, False),
                                          ("trailview-ground", TileType.AVIF, avif, False)]:
            with self.subTest(kind=kind, tile_type=tile_type, ok=ok), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "archive.pmtiles"
                with write(path) as writer:
                    writer.write_tile(zxy_to_tileid(9, 0, 0), data)
                    writer.finalize({"tile_type": tile_type, "tile_compression": Compression.NONE, "min_zoom": 9, "max_zoom": 9},
                                    {"trailview_format": 1})
                if ok:
                    planner_verify.archive(path, kind)
                else:
                    with self.assertRaisesRegex(ValueError, "tile (format|size)"):
                        planner_verify.archive(path, kind)

    def test_trailview_feature_tiles_decode_as_the_shared_vectors(self):
        vectors = Path(__file__).resolve().parents[2] / "specs/vectors"
        decoded = json.loads((vectors / "trailview-features.json").read_text())
        self.assertEqual(planner_verify.features((vectors / "trailview-features.bin").read_bytes()), decoded)
        self.assertEqual(planner_verify.features(b"\x00\x00"), [])
        with self.assertRaisesRegex(ValueError, "bytes after"):
            planner_verify.features((vectors / "trailview-features.bin").read_bytes() + b"\x00")

    def test_changed_binary_archives_check_the_decoded_consumer_layouts(self):
        for kind, zoom, size, metadata in [('snow-seasons', 8, 2 * 2 * 64 * 64, {'seasons': 2}),
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
