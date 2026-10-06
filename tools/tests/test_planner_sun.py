"""The sun index bounds every bilinear patch and retains unknown terrain."""
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
from PIL import Image
from tools import planner_sun as sun
from tools import planner_sun_horizons as horizons
from tools import planner_grid_maps as grid, planner_offline as offline


class SunIndexTest(unittest.TestCase):
    def terrain(self, root, tiles=False):
        source = root / "terrain.mbtiles"
        with sqlite3.connect(source) as db:
            db.executescript("CREATE TABLE metadata(name TEXT, value TEXT);"
                             "CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
            db.executemany("INSERT INTO metadata VALUES (?,?)", [("format", "webp"), ("minzoom", "0"),
                ("maxzoom", "12"), ("bounds", "7,47,9,49"), ("attribution", "Terrain credit")])
            if tiles:
                db.execute("INSERT INTO tiles VALUES (12,0,0,?)", (sun.encode(np.zeros((2, 2), np.int16)),))
        output = root / "output"
        output.mkdir()
        return source, {"output": str(output), "layers": {"planner/terrain": {source.name: str(source)}},
            "options": {"bounds": [7.9,47.9,8.1,48.1], "time_zone": "Europe/Berlin", "distance_m": 30000,
                        "horizon_samples": 32, "horizon_directions": 72}}

    def test_empty_terrain_keeps_unknown_sun_coverage_without_an_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, request = self.terrain(root)
            with patch.object(sun, "bake", side_effect=AssertionError("Empty terrain must not bake tiles")):
                sun.step(request)
            output = Path(request["output"])
            self.assertEqual([path.relative_to(output).as_posix() for path in output.rglob("*") if path.is_file()],
                             ["sun/empty.json"])
            metadata = json.loads((output / "sun/empty.json").read_bytes())
            self.assertEqual((metadata["sun_format"], metadata["coverage"], metadata["terrain_sha256"]),
                             (3, [7,47,9,49], offline.item(source)["sha256"]))
            self.assertEqual((metadata["horizon_step"], metadata["attribution"]), (horizons.ANGLE_STEP, "Terrain credit"))
            for kind, paths in [("terrain", {source.name: str(source)}),
                                ("sun", {"sun/empty.json": str(output / "sun/empty.json")})]:
                packed = root / kind
                packed.mkdir()
                grid.step({"output": str(packed), "metrics": str(root / f"{kind}.metrics.json"),
                           "layers": {f"planner/{kind}": paths}, "options": {"kind": kind, "bounds": request["options"]["bounds"]}})
                index = json.loads((packed / "index.json").read_bytes())
                self.assertEqual(set(index["files"]), {f"maps/{kind}.json"})

    def test_nonempty_step_uses_the_same_converted_terrain_bytes(self):
        from pmtiles.convert import mbtiles_to_pmtiles
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, request = self.terrain(root, tiles=True)
            expected = root / "expected.pmtiles"
            mbtiles_to_pmtiles(source, expected, None)
            def bake(terrain, output, *options):
                self.assertEqual(terrain.read_bytes(), expected.read_bytes())
                self.assertEqual(output.relative_to(Path(request["output"])).as_posix(), "sun/sun.pmtiles")
                output.write_bytes(b"baked archive")
            with patch.object(sun, "bake", side_effect=bake):
                sun.step(request)
            self.assertEqual((Path(request["output"]) / "sun/sun.pmtiles").read_bytes(), b"baked archive")
            self.assertEqual(list(Path(request["output"]).iterdir()), [Path(request["output"]) / "sun"])

    def test_missing_or_invalid_empty_terrain_is_an_error(self):
        for invalid in ("missing", "database", "format", "coverage", "bounds"):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as temporary:
                source, request = self.terrain(Path(temporary))
                if invalid == "missing":
                    source.unlink()
                elif invalid == "database":
                    source.write_bytes(b"invalid")
                else:
                    with sqlite3.connect(source) as db:
                        db.execute("UPDATE metadata SET value=? WHERE name=?",
                                   ("png", "format") if invalid == "format" else
                                   ("invalid", "bounds") if invalid == "bounds" else ("7.9,47.9,8.1,48.1", "bounds"))
                with self.assertRaises((sqlite3.Error, ValueError)):
                    sun.step(request)

    def test_bounds_include_shared_far_edges_and_unknown_vertices(self):
        vertices = np.zeros((9, 9), np.int16)
        vertices[4, 4] = 1400
        self.assertEqual(sun.maxima(vertices, 4).tolist(), [[1400, 1400], [1400, 1400]])
        vertices[8, 8] = sun.UNKNOWN
        self.assertEqual(sun.maxima(vertices, 4).tolist(), [[1400, 1400], [1400, sun.UNKNOWN]])

    def test_bound_quantization_is_conservative_at_signed_extrema(self):
        values = np.array([-32768, -301, -1, 0, 1400, 32766, sun.UNKNOWN], np.int16)
        rounded = sun.quantize(values)
        self.assertTrue((rounded >= values).all())
        self.assertEqual(rounded[-1], sun.UNKNOWN)
        self.assertTrue((rounded[:-2].astype(np.int32) - values[:-2] < sun.BOUND_STEP).all())

    def test_signed_heights_and_the_unknown_sentinel_survive_webp(self):
        values = np.array([[-300, 0, 1], [1400, -32768, sun.UNKNOWN]], np.int16)
        rgba = np.asarray(Image.open(io.BytesIO(sun.encode(values))).convert('RGBA'))
        decoded = rgba[:, :, 0].astype(np.int32) * 256 + rgba[:, :, 1] - 32768
        np.testing.assert_array_equal(decoded, values)
        self.assertTrue((rgba[:, :, 2] == 0).all())
        self.assertTrue((rgba[:, :, 3] == 255).all())

    def test_horizon_delta_planes_round_trip_including_unknown_directions(self):
        values = np.random.default_rng(7).integers(0, 256, (8, 8, 72), dtype=np.uint8)
        np.testing.assert_array_equal(horizons.decode(horizons.encode(values), 8, 72), values)

    def test_overview_shares_a_tile_with_height_bounds_without_changing_either(self):
        values = np.random.default_rng(7).integers(0, 256, (32, 32, 72), dtype=np.uint8)
        heights = np.full((512, 512), -300, np.int16)
        heights[-1, -1] = sun.UNKNOWN
        bounds = np.asarray(Image.open(io.BytesIO(sun.encode(heights))).convert('RGBA'))
        data = horizons.encode(values, bounds)
        np.testing.assert_array_equal(horizons.decode(data, 32, 72), values)
        rgba = np.asarray(Image.open(io.BytesIO(data)).convert('RGBA'))
        np.testing.assert_array_equal(rgba[:512], bounds)
        self.assertEqual(rgba.shape, (560, 512, 4))

    def test_overview_averages_round_up_and_keep_unknown_children(self):
        children = np.array([[[0, 255], [1, 2]], [[1, 2], [3, 2]]], np.uint8)
        np.testing.assert_array_equal(horizons.reduce_grid(children), [[[2, 255]]])

    def test_horizon_includes_an_interior_peak_and_preserves_missing_terrain(self):
        arrays = np.array([0, 10, 10, 0] + [10] * 12, np.int16)
        origins = np.zeros((13, 5), np.int64)
        origins[0, 2:] = [2, 2, 0]
        for level in range(1, 13):
            origins[level, 2:] = [1, 1, level + 3]
        value = horizons.horizon(arrays, origins, 0., 0., 1., 1., 1.)
        expected = np.degrees(np.arctan(20 - 2 * np.sqrt(20)))
        self.assertLessEqual(expected, value * horizons.ANGLE_STEP)
        self.assertLess(value * horizons.ANGLE_STEP - expected, horizons.ANGLE_STEP)
        arrays[3] = horizons.UNKNOWN
        self.assertEqual(horizons.horizon(arrays, origins, 0., 0., 1., 1., 1.), 255)
