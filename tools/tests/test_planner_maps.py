"""Map archives cover their contour neighbours and a bundle check detects changed files."""

import hashlib
import io
import json
import math
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_maps as maps
from tools import planner_map_archive as archive


class PlannerMaps(unittest.TestCase):
    def test_terrain_covers_contour_neighbours_at_every_supported_zoom(self):
        for region in [[5.95, 45.8, 10.5, 49.85], [148.8, -37.2, 149.1, -36.5]]:
            west, south, east, north = maps.terrain_bounds(region)
            for zoom in range(10, 16):
                count = 1 << zoom
                for lon in [region[0], region[2]]:
                    for lat in [region[1], region[3]]:
                        x = math.floor((lon + 180) / 360 * count)
                        y = math.floor((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * count)
                        for dx in [-1, 0, 1]:
                            for dy in [-1, 0, 1]:
                                scale = 1 << max(0, zoom - 12)
                                dem_x, dem_y = (x + dx) // scale, (y + dy) // scale
                                dem_count = count // scale
                                center_lon = (dem_x + 0.5) / dem_count * 360 - 180
                                center_lat = math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * (dem_y + 0.5) / dem_count))))
                                self.assertTrue(west < center_lon < east and south < center_lat < north,
                                                (region, zoom, dem_x, dem_y))

    def test_full_bundle_check_refuses_a_changed_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            (destination / "basemap.pmtiles").write_bytes(b"base")
            (destination / "manifest.json").write_text(json.dumps({"files": {"basemap.pmtiles": {
                "bytes": 4, "sha256": hashlib.sha256(b"base").hexdigest()}}}))
            maps.check_bundle(destination, full=True)
            (destination / "basemap.pmtiles").write_bytes(b"oops")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                maps.check_bundle(destination, full=True)

    def test_per_zoom_halo_covers_contour_overzoom_without_growing_high_zoom_padding(self):
        for region in [[5.95, 45.8, 10.5, 49.85], [148.8, -37.2, 149.1, -36.5], [-180, 0, -179, 1], [179, -1, 180, 0]]:
            for zoom in range(10, 16):
                left, top, right, bottom = archive.tile_window(region, zoom)
                scale = 1 << max(0, zoom - 12)
                dem_zoom = min(zoom, 12)
                for x in (left, right - 1):
                    for y in (top, bottom - 1):
                        for dx in (-1, 0, 1):
                            for dy in (-1, 0, 1):
                                dem_x = ((x + dx) // scale) % (1 << dem_zoom)
                                dem_y = (y + dy) // scale
                                if 0 <= dem_y < (1 << dem_zoom):
                                    self.assertTrue(archive.selected((dem_zoom, dem_x, dem_y), region, terrain=True))
            left, top, right, bottom = archive.tile_window(region, 12)
            self.assertFalse(archive.selected((12, (left - 2) % 4096, top), region, terrain=True))
            self.assertFalse(archive.selected((12, left, bottom + 1), region, terrain=True))

    def test_tile_boundary_has_no_extra_intersecting_tile(self):
        latitude = lambda y: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / 16))))
        self.assertEqual(archive.tile_window([0, latitude(8), 22.5, latitude(7)], 4), (8, 7, 9, 8))

    def test_archive_roundtrip_preserves_transparent_rgb_and_deduplicated_vector_tiles(self):
        from PIL import Image
        from pmtiles.reader import Reader
        from pmtiles.tile import Compression, TileType, zxy_to_tileid
        from pmtiles.writer import write
        rgba = bytes([128, 0, 0, 0, 128, 1, 255, 255, 127, 254, 1, 127, 255, 255, 255, 0])
        image = Image.frombytes("RGBA", (2, 2), rgba)
        data = io.BytesIO()
        image.save(data, format="WEBP", lossless=True, exact=True)
        encoded, exact = archive.lossless(data.getvalue())
        self.assertEqual(exact[8:], rgba)
        self.assertEqual(archive.pixels(encoded)[8:], rgba)
        with tempfile.TemporaryDirectory() as temporary:
            for terrain, payload in [(True, data.getvalue()), (False, b"vector payload")]:
                source, target = Path(temporary) / f"{terrain}-source.pmtiles", Path(temporary) / f"{terrain}-target.pmtiles"
                header = {"tile_type": TileType.WEBP if terrain else TileType.MVT,
                          "tile_compression": Compression.NONE,
                          "min_lon_e7": -1800000000, "min_lat_e7": -850000000,
                          "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                          "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}
                with write(source) as writer:
                    for zxy in [(0, 0, 0), (1, 0, 0), (1, 1, 0)]:
                        writer.write_tile(zxy_to_tileid(*zxy), payload)
                    writer.finalize(header, {"attribution": "Test source"})
                result = archive.extract(source, target, [1, 1, 2, 2], terrain=terrain)
                self.assertEqual(result["verified_tiles"], 3 if terrain else 2)
                self.assertEqual(result["output_bytes"], target.stat().st_size)
                content = target.read_bytes()
                reader = Reader(lambda offset, length: content[offset:offset + length])
                self.assertEqual(reader.metadata(), {"attribution": "Test source"})
                actual = reader.get(1, 1, 0)
                self.assertEqual(archive.pixels(actual) if terrain else actual,
                                 archive.pixels(payload) if terrain else payload)
                with self.assertRaisesRegex(ValueError, "already exists"):
                    archive.extract(source, target, [1, 1, 2, 2], terrain=terrain)
                copied = Path(temporary) / f"{terrain}-copied.pmtiles"
                with patch.object(archive, "lossless", side_effect=AssertionError("Tile was recompressed")):
                    copy = archive.extract(source, copied, [1, 1, 2, 2], terrain=terrain, recompress=False)
                self.assertEqual(copy["content_sha256"], result["content_sha256"])


if __name__ == "__main__":
    unittest.main()
