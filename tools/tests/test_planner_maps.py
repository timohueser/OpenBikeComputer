"""A map bundle is visible only after both archives and its assets are ready."""

import argparse
import hashlib
import io
import json
import math
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from tools import planner_maps as maps


def assets():
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w") as archive:
        for name in ["fonts/OFL.txt", "fonts/Noto Sans Regular/0-255.pbf",
                     "sprites/v4/light.json", "sprites/v4/dark@2x.png"]:
            archive.writestr(f"assets-root/{name}", b"asset")
    return data.getvalue()


class PlannerMaps(unittest.TestCase):
    def test_terrain_covers_contour_neighbours_at_every_supported_zoom(self):
        for region in [maps.bounds(maps.BW_BOUNDS), [148.8, -37.2, 149.1, -36.5]]:
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

    def test_failed_download_never_publishes_a_partial_bundle(self):
        args = argparse.Namespace(pmtiles="pmtiles", basemap="base", terrain="dem",
                                  bbox=maps.bounds(maps.BW_BOUNDS))
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "planner"
            with patch.object(maps, "DATA", destination), \
                 patch.object(maps, "run", side_effect=OSError("download interrupted")):
                with self.assertRaisesRegex(OSError, "interrupted"):
                    maps.prepare(args)
            self.assertFalse(destination.exists())
            self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_complete_bundle_records_sources_and_file_hashes_and_refuses_replacement(self):
        args = argparse.Namespace(pmtiles="pmtiles", basemap="base", terrain="dem",
                                  bbox=maps.bounds(maps.BW_BOUNDS))

        def extract(*command):
            expected = maps.terrain_bounds(args.bbox) if command[2] == "dem" else args.bbox
            self.assertEqual(command[4], "--bbox=" + ",".join(map(str, expected)))
            Path(command[3]).write_bytes(command[2].encode())

        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "planner"
            with patch.object(maps, "DATA", destination), \
                 patch.object(maps, "run", side_effect=extract), \
                 patch.object(maps, "verify_archive"), \
                 patch.object(maps, "urlopen", side_effect=[io.BytesIO(assets()), io.BytesIO(b"MIT licence")]):
                maps.prepare(args)
                with self.assertRaisesRegex(ValueError, "already exists"):
                    maps.prepare(args)
            manifest = json.loads((destination / "manifest.json").read_text())
            self.assertEqual(manifest["sources"]["basemap"], "base")
            self.assertEqual(manifest["bounds"], [7.45, 47.5, 10.5, 49.85])
            self.assertEqual(manifest["terrain_bounds"], maps.terrain_bounds(args.bbox))
            self.assertEqual(manifest["files"]["basemap.pmtiles"], {
                "bytes": 4, "sha256": hashlib.sha256(b"base").hexdigest(),
            })
            self.assertIn("assets/fonts/OFL.txt", manifest["files"])
            self.assertEqual((destination / "assets/sprites/LICENSE.txt").read_bytes(), b"MIT licence")
            self.assertEqual(list(Path(temporary).iterdir()), [destination])


if __name__ == "__main__":
    unittest.main()
