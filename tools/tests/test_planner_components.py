"""POI updates reuse addresses, other producers, and their grid packaging."""

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from tools import planner_bake as bake, planner_blocks as blocks, planner_components as components
from tools import planner_grid_components as grid, planner_prepare as preparation, planner_runtime as runtime


BOUNDS = [7.75, 48, 7.76, 48.01]


def database(path, component, name="Bakery"):
    sys.path.insert(0, str(bake.SEARCH))
    try:
        from writer import Writer
        writer = Writer(path.stem, path.parent, component)
        writer.add({"osm_key": "shop", "osm_value": "bakery", "object_type": "N", "object_id": 1,
            "name": {"name": name}, "centroid": [7.755, 48.005], "address": {"street": "Main", "city": "Town"},
            "housenumber": "4", "country_code": "de", "extra": {"description": name}})
        writer.finish({"component": component, "bounds": BOUNDS, "osm_sha256": "a" * 64, "attribution": "OSM"})
    finally:
        sys.path.pop(0)


def sample_release(root, changed=False):
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write
    root.mkdir()
    (root / "maps").mkdir()
    package = "b" * 64
    for kind in ["basemap", "places", "overlays", "terrain"]:
        with write(root / f"maps/{kind}.pmtiles") as writer:
            writer.write_tile(zxy_to_tileid(0, 0, 0), b"\x1a\x08\x0a\x06places")
            writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.NONE,
                "min_lon_e7": -1800000000, "min_lat_e7": -850000000, "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}, {"routing_package": package} if kind == "overlays" else {})
    (root / "routing").mkdir()
    with closing(sqlite3.connect(root / "routing/overlays.sqlite")) as db:
        db.executescript("CREATE TABLE metadata(package TEXT,bounds TEXT); CREATE TABLE attributes(id INTEGER PRIMARY KEY,properties TEXT);"
            "CREATE TABLE routes(id INTEGER PRIMARY KEY,properties TEXT); CREATE TABLE geometries(id INTEGER PRIMARY KEY);"
            "CREATE TABLE features(id INTEGER PRIMARY KEY,geometry INTEGER,attributes INTEGER);")
        db.commit()
    for component in ["pois", "addresses"]:
        path = root / f"search/{component}/test.sqlite"
        path.parent.mkdir(parents=True)
        database(path, component, "Changed bakery" if changed and component == "pois" else "Bakery")
    font = root / "maps/assets/fonts/Noto Sans Regular/0-255.pbf"
    font.parent.mkdir(parents=True)
    font.write_bytes(b"glyphs")
    model = root / "search/model/labels.json"
    model.parent.mkdir()
    model.write_bytes(b"{}")
    routes = root / "routes/test.json"
    routes.parent.mkdir()
    cell = next(iter(blocks.cells(BOUNDS)))[0]
    routes.write_bytes(runtime.encoded({"format": 1, "routes": [{"id": 123, "kind": "hiking", "name": "Tiny trail",
        "cells": [cell], "line_udeg": [7755000, 48005000, 100, 0], "via": []}]}))
    document = {"format": 1, "region": "test", "bounds": BOUNDS, "terrain_bounds": BOUNDS, "osm_sha256": "a" * 64,
        "routing_package": package, "profiles": ["touring"], "sources": {},
        "files": {path.relative_to(root).as_posix(): {"bytes": path.stat().st_size, "sha256": runtime.digest(path)}
                  for path in root.rglob("*") if path.is_file()}, "source_files": {}}
    (root / "release.json").write_bytes(runtime.encoded(document))
    return document


class ComponentTests(unittest.TestCase):
    def test_component_keys_include_invoked_producers_and_local_rust_dependencies(self):
        config = preparation.recipe(bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
        original = bake.specifications(config)
        digest = components.digest
        cases = {
            "tools/planner_places.py": {"places"},
            "builder/app/src/lib/planner/poi-kinds.json": {"places"},
            "tools/planner_overlays.py": {"overlays"},
            "tools/planner_map_archive.py": {"terrain", "sun"},
            "firmware/obc-elevation/src/grid.rs": {"terrain", "routing", "overlays", "sun"},
            "firmware/obc-formats/Cargo.toml": {"terrain", "routing", "overlays", "sun"},
            "tools/planner_sun_horizons.py": {"sun"},
        }
        for filename, expected in cases.items():
            with self.subTest(filename=filename):
                changed_path = bake.maps.ROOT / filename
                self.assertTrue(changed_path.exists())
                with patch.object(components, "digest", side_effect=lambda path: "changed" if path == changed_path else digest(path)):
                    changed = bake.specifications(config)
                self.assertEqual({name for name in original if original[name] != changed[name]}, expected)

    def test_composition_does_not_write_through_the_previous_device_catalogue(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            previous = root / "previous"
            document = sample_release(previous)
            catalogue = previous / "device/catalog.json"
            catalogue.parent.mkdir()
            catalogue.write_bytes(b'{"url":"https://maps.test/catalog.json"}\n')
            before = catalogue.read_bytes()
            catalogue.chmod(0o444)
            terrain = previous / "maps/provenance.json"
            terrain.write_bytes(runtime.encoded({"terrain_sources": [], "terrain_attribution": "Terrain"}))
            built = {}
            for name in ("basemap", "places", "overlays", "terrain", "assets", "routing", "model", "pois", "addresses"):
                folder = previous / ("maps" if name in {"basemap", "places", "overlays", "terrain", "assets"} else
                                     f"search/{name}" if name in {"pois", "addresses"} else "search" if name == "model" else name)
                filenames = [f"{name}.pmtiles"] if name in {"basemap", "places", "overlays", "terrain"} else \
                    ["assets/fonts/Noto Sans Regular/0-255.pbf"] if name == "assets" else \
                    ["model/labels.json"] if name == "model" else ["test.sqlite"] if name in {"pois", "addresses"} else ["overlays.sqlite"]
                built[name] = (folder, {"files": {filename: {"bytes": (folder / filename).stat().st_size,
                    "sha256": runtime.digest(folder / filename)} for filename in filenames}})
            args = argparse.Namespace(data_dir=root / "updated", source_cache=root / "cache", osm=None,
                recipe=bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json", device_catalog="https://maps.test/catalog.json")
            config = {"region": "test", "bounds": BOUNDS, "osm": {"sha256": document["osm_sha256"]}, "probe": {}}
            def seal(stage, *_args):
                copied = stage / "device/catalog.json"
                self.assertNotEqual(copied.stat().st_ino, catalogue.stat().st_ino)
                copied.write_bytes(b'{"url":"normalized"}\n')
                return "release-id", {}
            with patch.object(bake.releases, "seal", side_effect=seal):
                bake.compose(args, config, built, previous)
            self.assertEqual(catalogue.read_bytes(), before)
            self.assertEqual((args.data_dir / "device/catalog.json").read_bytes(), b'{"url":"normalized"}\n')

    def test_shared_elevation_changes_select_routing_but_visual_terrain_edits_do_not(self):
        config = preparation.recipe(bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
        original = bake.specifications(config)
        previous = {name: {"spec": spec} for name, spec in original.items()}
        changed_config = json.loads(json.dumps(config))
        tile = next(iter(changed_config["terrain"]["dem"]))
        changed_config["terrain"]["dem"][tile] = "d" * 64
        changed = bake.specifications(changed_config)
        for selected in ("terrain", "routing"):
            _, active = components.plan(changed, [selected], previous)
            self.assertEqual(active, {"terrain", "routing", "overlays", "sun"})
        digest = components.digest
        for filename, expected in [("firmware/obc-elevation/src/grid.rs", {"terrain", "routing", "overlays", "sun"}),
                                   ("tools/planner_map_archive.py", {"terrain", "sun"})]:
            with self.subTest(filename=filename):
                with patch.object(components, "digest", side_effect=lambda path: "changed" if path == bake.maps.ROOT / filename else digest(path)):
                    changed = bake.specifications(config)
                _, active = components.plan(changed, ["terrain"], previous)
                self.assertEqual(active, expected)

    def test_sunlight_receives_its_terrain_dependency_and_extends_context(self):
        config = preparation.recipe(bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
        specs = bake.specifications(config)
        self.assertEqual(specs["sun"]["dependencies"], ["terrain"])
        context = bake.terrain_coverage(config)
        for edge in (0, 1): self.assertLess(context[edge], config["bounds"][edge])
        for edge in (2, 3): self.assertGreater(context[edge], config["bounds"][edge])
        without_sun = {key: value for key, value in config.items() if key != "sun"}
        self.assertNotEqual(specs["terrain"], bake.specifications(without_sun)["terrain"])
        with tempfile.TemporaryDirectory() as temporary:
            terrain = Path(temporary) / "terrain"
            stage = Path(temporary) / "sun"
            with patch.object(bake.maps, "run") as run, patch.object(bake.releases, "archive_metadata", return_value={"sun_format": 3}):
                bake.build_layer(stage, config, "sun", terrain)
            command = run.call_args.args
            self.assertEqual(command[command.index("--terrain") + 1], terrain / "terrain.pmtiles")

    def test_receipts_are_verified_atomic_and_reused(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache = components.Cache(Path(temporary))
            spec = components.specification("pois", "one", {"osm": "a" * 64}, {}, BOUNDS)
            def fail(stage):
                (stage / "partial").write_bytes(b"partial")
                raise OSError("interrupted")
            with self.assertRaises(OSError): cache.build(spec, fail)
            self.assertEqual(cache.status(spec)[0], "missing")
            root, receipt = cache.build(spec, lambda stage: (stage / "data").write_bytes(b"complete"))
            self.assertGreaterEqual(receipt["cost"]["elapsed_seconds"], 0)
            self.assertEqual(receipt["cost"]["output_bytes"], 8)
            self.assertIsNone(receipt["cost"]["peak_ram_bytes"])
            cache.build(spec, lambda stage: self.fail("A verified producer ran twice"))
            (root / "data").chmod(0o644)
            (root / "data").write_bytes(b"corrupt!")
            with self.assertRaisesRegex(ValueError, "checksum"): cache.build(spec, lambda _: None)

    def test_producer_keys_and_selection_keep_address_work_out_of_a_poi_update(self):
        config = preparation.recipe(bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
        original = bake.specifications(config)
        digest = components.implementation
        def poi_change(functions=(), paths=()):
            value = digest(functions, paths)
            return "changed-poi-transform" if bake.SEARCH / "pois.py" in paths else value
        with patch.object(components, "implementation", side_effect=poi_change):
            changed = bake.specifications(config)
        self.assertNotEqual(changed["pois"], original["pois"])
        for name in set(original) - {"pois"}: self.assertEqual(changed[name], original[name], name)
        previous = {name: {"spec": spec} for name, spec in original.items()}
        selected, active = components.plan(changed, ["pois"], previous)
        self.assertEqual(active, {"pois", "source-records", "source-search"})
        self.assertEqual(selected["addresses"], original["addresses"])
        _, routing = components.plan(changed, ["routing"], previous)
        self.assertEqual(routing, {"routing", "overlays"})
        _, basemap = components.plan(changed, ["basemap"], previous)
        self.assertEqual(basemap, {"basemap", "places", "source-basemap"})
        with tempfile.TemporaryDirectory() as temporary:
            cache = components.Cache(Path(temporary))
            for spec in original.values():
                cache.build(spec, lambda stage: (stage / "fixture").write_bytes(b"pinned source or component"))
            args = argparse.Namespace(osm=None, inputs=None)
            def build_pois(stage, records, config, component):
                self.assertEqual(component, "pois")
                (stage / "new-pois").write_bytes(b"metadata")
            with patch.object(bake, "build_search", side_effect=build_pois) as build, patch.object(bake.maps, "run") as run:
                bake.execute(args, config, cache, selected, active)
                self.assertEqual(build.call_count, 1)
                run.assert_not_called()

    def test_a_failed_producer_stops_the_producer_that_runs_beside_it(self):
        config = preparation.recipe(bake.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
        specs = {name: spec for name, spec in bake.specifications(config).items() if name in ("terrain", "assets")}
        def fail(stage): raise ValueError("assets failed")
        with tempfile.TemporaryDirectory() as temporary, patch.object(bake, "build_assets", side_effect=fail), \
                patch.object(bake, "build_terrain", side_effect=lambda *_: bake.maps.run("sleep", "60")):
            start = time.monotonic()
            with self.assertRaisesRegex(ValueError, "assets failed"):
                bake.execute(argparse.Namespace(osm=None, inputs=None), config, components.Cache(Path(temporary)), specs, set(specs))
        self.assertLess(time.monotonic() - start, 30)
        self.assertFalse(bake.maps.RUNNING)

    def test_grid_poi_update_does_not_partition_or_compress_other_components(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "first"
            sample_release(source)
            cache = components.Cache(root / "cache")
            def route_blocks(*command, **kwargs):
                if Path(str(command[0])).name != "route-blocks": return
                output = Path(command[2]); output.mkdir()
                cells = json.loads(Path(command[4]).read_bytes())
                (output / "blocks.json").write_bytes(runtime.encoded({"source": "b" * 64, "archives": []}))
                catalog = []
                for cell in cells:
                    manifest = cell["id"] + ".json"
                    (output / manifest).write_bytes(runtime.encoded({"bounds": cell["bounds"]}))
                    catalog.append({**cell, "manifest": manifest})
                (output / "catalog.json").write_bytes(runtime.encoded({"cells": catalog}))
            with patch.object(grid.maps, "run", side_effect=route_blocks):
                catalogue = grid.publish(source, None, root / "grid-first", cache)
            self.assertTrue(all(f"routes/tiles/{cell['id']}.json" in cell["files"] for cell in catalogue["cells"]))
            before = list(cache.inventory())
            updated = root / "updated"
            sample_release(updated, changed=True)
            with patch.object(grid.maps, "run") as run:
                grid.publish(updated, None, root / "grid-updated", cache)
                run.assert_not_called()
            after = list(cache.inventory())
            old = {(item["component"], item["key"]) for item in before}
            added = [item for item in after if (item["component"], item["key"]) not in old]
            self.assertTrue(added)
            self.assertTrue(all(item["component"].startswith(("grid-search-pois", "grid-search-lookup-pois")) for item in added), added)
            _, first = runtime.release(root / "grid-first")
            _, second = runtime.release(root / "grid-updated")
            changed_files = {name for name in first["files"] if first["files"][name] != second["files"].get(name)}
            self.assertTrue(any(name.startswith("search/tiles/pois/") for name in changed_files))
            self.assertTrue(all(name.startswith(("search/tiles/pois/", "offline/catalog.json", "search/test.grid.json")) for name in changed_files), changed_files)


if __name__ == "__main__": unittest.main()
