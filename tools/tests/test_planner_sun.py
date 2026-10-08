"""The sun index bounds every bilinear patch and retains unknown terrain."""
import argparse
import hashlib
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
from tools import planner_grid_maps as grid, planner_grid_index, planner_offline as offline, planner_runtime as runtime


class SunIndexTest(unittest.TestCase):
    def terrain(self, root, tiles=False):
        source = root / "terrain.mbtiles"
        with sqlite3.connect(source) as db:
            db.executescript("CREATE TABLE metadata(name TEXT, value TEXT);"
                             "CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
            db.executemany("INSERT INTO metadata VALUES (?,?)", [("format", "webp"), ("minzoom", "0"),
                ("maxzoom", "12"), ("bounds", "7,47,9,49"), ("attribution", "Terrain credit"),
                ("source_sha256", '["dem"]')])
            if tiles:
                db.executemany("INSERT INTO tiles VALUES (12,?,0,?)",
                               [(x, sun.encode(np.full((2, 2), x, np.int16))) for x in (0, 4)])
        output = root / "output"
        output.mkdir()
        return source, {"output": str(output), "layers": {"planner/terrain": {source.name: str(source)}},
            "options": {"bounds": [7.9,47.9,8.1,48.1], "time_zone": "Europe/Berlin", "distance_m": 30000,
                        "horizon_samples": 32, "horizon_directions": 72}}

    def grid_request(self, root, source, request):
        packed = root / "terrain-grid"
        packed.mkdir()
        grid.step({"output": str(packed), "metrics": str(root / "terrain.metrics.json"),
                   "layers": {"planner/terrain": {source.name: str(source)}},
                   "options": {"kind": "terrain", "bounds": [7,47,9,49]}})
        request["layers"] = {"planner/terrain/grid": {
            path.relative_to(packed).as_posix(): str(path) for path in packed.rglob("*") if path.is_file()}}
        index = json.loads((packed / "index.json").read_bytes())
        source.unlink()
        return packed, index

    def test_empty_terrain_keeps_unknown_sun_coverage_without_an_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, request = self.terrain(root)
            _, index = self.grid_request(root, source, request)
            with patch.object(sun, "bake", side_effect=AssertionError("Empty terrain must not bake tiles")):
                sun.step(request)
            output = Path(request["output"])
            self.assertEqual([path.relative_to(output).as_posix() for path in output.rglob("*") if path.is_file()],
                             ["sun/empty.json"])
            metadata = json.loads((output / "sun/empty.json").read_bytes())
            self.assertEqual((metadata["sun_format"], metadata["coverage"], metadata["terrain_grid_sha256"]),
                             (3, [7,47,9,49], hashlib.sha256(runtime.encoded(index)).hexdigest()))
            self.assertNotIn("terrain_sha256", metadata, "the step consumes grid bytes, not the original archive")
            self.assertEqual((metadata["horizon_step"], metadata["attribution"]), (horizons.ANGLE_STEP, "Terrain credit"))
            packed = root / "sun-grid"
            packed.mkdir()
            grid.step({"output": str(packed), "metrics": str(root / "sun.metrics.json"),
                       "layers": {"planner/sun": {"sun/empty.json": str(output / "sun/empty.json")}},
                       "options": {"kind": "sun", "bounds": request["options"]["bounds"]}})
            self.assertEqual(set(json.loads((packed / "index.json").read_bytes())["files"]), {"maps/sun.json"})

    def test_nonempty_step_reconstructs_verified_terrain_tile_bytes(self):
        from pmtiles.convert import mbtiles_to_pmtiles
        from pmtiles.reader import MmapSource, Reader, all_tiles
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, request = self.terrain(root, tiles=True)
            expected = root / "expected.pmtiles"
            mbtiles_to_pmtiles(source, expected, None)
            with expected.open("rb") as stream:
                read = MmapSource(stream)
                original_header, tiles = Reader(read).header(), dict(all_tiles(read))
            packed, index = self.grid_request(root, source, request)
            self.assertEqual(sum(name.endswith(".pmtiles") for name in index["files"]), 2)
            def bake(terrain, output, *options, terrain_grid_sha256):
                with terrain.open("rb") as stream:
                    read = MmapSource(stream)
                    self.assertEqual(dict(all_tiles(read)), tiles)
                    header = Reader(read).header()
                    for key in ("tile_type", "tile_compression", "min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7"):
                        self.assertEqual(header[key], original_header[key])
                self.assertEqual(terrain_grid_sha256, hashlib.sha256(runtime.encoded(index)).hexdigest())
                self.assertEqual(output.relative_to(Path(request["output"])).as_posix(), "sun/sun.pmtiles")
                output.write_bytes(b"baked archive")
            with patch.object(sun, "bake", side_effect=bake):
                sun.step(request)
            self.assertEqual((Path(request["output"]) / "sun/sun.pmtiles").read_bytes(), b"baked archive")
            self.assertEqual(list(Path(request["output"]).iterdir()), [Path(request["output"]) / "sun"])
            entry = next(value for name, value in index["files"].items() if name.endswith(".pmtiles"))
            (packed / "objects" / entry["transport"]["sha256"]).write_bytes(b"tampered")
            request["output"] = str(root / "retry")
            Path(request["output"]).mkdir()
            with patch.object(sun, "bake", side_effect=AssertionError("Tampered grid must not bake")):
                with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
                    sun.step(request)
            self.assertFalse((Path(request["output"]) / "sun/sun.pmtiles").exists())

    def test_grid_composition_requires_the_exact_consumed_terrain_manifest(self):
        from tools import planner_grid
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, request = self.terrain(root)
            _, terrain = self.grid_request(root, source, request)
            sun.step(request)
            metadata = json.loads((Path(request["output"]) / "sun/empty.json").read_bytes())
            options = {"bounds": request["options"]["bounds"], "region": "test"}
            item = {"bytes": 0, "sha256": "a" * 64, "transport": {
                "bytes": 0, "sha256": "a" * 64, "encoding": "identity"}}
            cells = [{"id": name, "bounds": bounds, "files": []}
                     for name, bounds in planner_grid.cells(options["bounds"])]
            def index(kind, **values):
                return {"format": 1, "kind": kind, "files": {}, **values}
            indexes = {kind: index(kind, metadata={"bounds": options["bounds"]})
                       for kind in ("basemap", "places", "overlays")}
            indexes["places"]["metadata"]["osm_sha256"] = "osm"
            indexes["overlays"]["metadata"]["routing_package"] = item["sha256"]
            indexes.update(terrain=terrain, sun=index("sun", metadata=metadata),
                           assets=index("assets"), fonts=index("fonts", aliases={}), model=index("model"))
            indexes["routing"] = index("routing", cells=[], files={"routing/blocks.json": item,
                **{f"routes/tiles/{cell['id']}.json": item for cell in cells}},
                graph={"source": "routing", "data": {**options, "source_sha256": ["osm", "dem"], "metrics": ["bike"]}})
            for component in ("pois", "addresses"):
                indexes[component] = index(component, cells=cells, metadata={**options, "schema": 6,
                    "time_zone": "Europe/Berlin", "osm_sha256": "osm", "component": component, "counts": {}})
            planner_grid_index.compose(indexes, options)
            terrain["source"]["sha256"] = "b" * 64
            with self.assertRaisesRegex(ValueError, "Sun uses another terrain grid"):
                planner_grid_index.compose(indexes, options)

    def test_missing_or_invalid_empty_terrain_is_an_error(self):
        for invalid in ("missing", "metadata", "format", "coverage", "bounds", "kind"):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source, request = self.terrain(root)
                packed, index = self.grid_request(root, source, request)
                if invalid == "missing":
                    entry = index["files"]["maps/terrain.json"]
                    (packed / "objects" / entry["transport"]["sha256"]).unlink()
                else:
                    if invalid == "kind":
                        index["kind"] = "sun"
                    elif invalid == "metadata":
                        index["metadata"]["attribution"] = "changed"
                    else:
                        metadata = dict(index["metadata"])
                        if invalid == "format": metadata["format"] = "png"
                        elif invalid == "bounds": metadata["bounds"] = "invalid"
                        else: metadata["bounds"] = request["options"]["bounds"]
                        path = root / "changed.json"
                        path.write_bytes(runtime.encoded(metadata))
                        index["files"]["maps/terrain.json"] = offline.pack_file(path, packed / "objects")
                        index["metadata"] = metadata
                    (packed / "index.json").write_bytes(runtime.encoded(index))
                    request["layers"]["planner/terrain/grid"] = {
                        path.relative_to(packed).as_posix(): str(path) for path in packed.rglob("*") if path.is_file()}
                with self.assertRaises((FileNotFoundError, ValueError, argparse.ArgumentTypeError)):
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
