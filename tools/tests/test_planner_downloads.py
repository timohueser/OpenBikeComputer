"""Area selections reuse published bytes, preserve coverage, and support resume."""

import gzip
import hashlib
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from tools import planner_downloads as downloads, planner_runtime as runtime, planner_offline as offline


class PlannerDownloads(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        files = {}
        for name, data in {"routing/packs/" + "c" * 64 + "/pages.bin": b"abcdef",
                           "routing/packs/" + "c" * 64 + "/pages.idx": b"index", "search/left.sqlite": b"left places",
                           "search/right.sqlite": b"right places", "maps/tiles/basemap/0-0-0.pmtiles": b"tiles",
                           "maps/tiles/places/0-0-0.pmtiles": b"places", "maps/tiles/overlays/6-33-22.pmtiles": b"networks", "offline/fonts/Sans.pbf": b"glyphs"}.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            files[name] = offline.pack_file(path, self.source / "objects")
        manifest = {"format": 1, "bounds": [7, 47, 9, 49], "region": "test", "profiles": ["road"]}
        graph = {"format": 2, "source": "b" * 64, "data": {"region": "test", "bounds": manifest["bounds"],
            "graph": {"head": {"len": 4, "pages": [0], "blocks": ["d" * 64]}}},
            "roads": [[0, 2]], "arcs": 4, "snap": {}, "archives": ["c" * 64]}
        cells = []
        for name, box in [("left", [7,47,8,49]), ("right", [8,47,9,49])]:
            path = self.source / "routing-cells" / (name + ".json")
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(runtime.encoded(graph))
            cells.append({"id": name, "bounds": box, "files": [f"search/{name}.sqlite"],
                "routing": {"manifest": f"routing-cells/{name}.json", "sha256": runtime.digest(path),
                    "adjacency": [[0,4]], "geometry_bounds": box}})
        publication = {"format": 3, "source": "a" * 64, "release": manifest, "zoom": 9, "map_zoom": 11,
            "routing_source": "b" * 64, "files": files,
            "shared": {f"maps/assets/fonts/Sans/{name}.pbf": "offline/fonts/Sans.pbf" for name in ("0-255", "256-511")},
            "map_blocks": [{"kind": "basemap", "tile": [0,0,0], "bounds": [-180,-85,180,85],
                            "files": ["maps/tiles/basemap/0-0-0.pmtiles"]},
                           {"kind": "places", "tile": [0,0,0], "bounds": [-180,-85,180,85],
                            "files": ["maps/tiles/places/0-0-0.pmtiles"]},
                           {"kind": "overlays", "tile": [6,33,22], "bounds": [5.625,45.09,11.25,48.92],
                            "files": ["maps/tiles/overlays/6-33-22.pmtiles"]}], "cells": cells}
        (self.source / "catalog.json").write_bytes(runtime.encoded(publication))
        self.service = downloads.Downloads(self.source, self.root / "cache", 1000000)

    def request(self, bounds=None):
        return {"bounds": bounds or [7,47,9,49]}

    def test_selection_reuses_static_objects_and_rounds_outward(self):
        before = {p.name: p.read_bytes() for p in (self.source / "objects").iterdir()}
        request = self.request([7.1, 47.1, 7.9, 48.9])
        first = self.service.prepare(request)
        self.assertEqual(first["state"], "ready")
        self.assertEqual(first, self.service.prepare(request))
        directory = self.service.cache / first["id"]
        manifest = json.loads((directory / "release.json").read_bytes())
        bundle = json.loads((directory / "bundle.json").read_bytes())
        self.assertEqual(manifest["bounds"], [7, 47, 8, 49])
        self.assertEqual(manifest["offline"]["cells"], [{"id": "left", "bounds": [7,47,8,49], "files": ["search/left.sqlite"]}])
        self.assertIn("search/left.sqlite", bundle["files"])
        self.assertIn("maps/tiles/places/0-0-0.pmtiles", bundle["files"])
        self.assertIn("maps/places.json", bundle["files"])
        self.assertNotIn("search/right.sqlite", bundle["files"])
        fonts = [bundle["files"][f"maps/assets/fonts/Sans/{name}.pbf"] for name in ("0-255", "256-511")]
        self.assertEqual(fonts, [self.service.publication["files"]["offline/fonts/Sans.pbf"]] * 2)
        self.assertEqual(manifest["files"]["maps/assets/fonts/Sans/0-255.pbf"]["bytes"], len(b"glyphs"))
        self.assertIn("maps/tiles/overlays/6-33-22.pmtiles", bundle["files"])
        overlays = bundle["files"]["maps/overlays.json"]["transport"]["sha256"]
        tilejson = json.loads(gzip.decompress((directory / "objects" / overlays).read_bytes()))
        self.assertEqual((tilejson["tiles"], tilejson["minzoom"]),
                         ([f"https://offline.openbikecomputer.invalid/{first['id']}/overlays/{{z}}/{{x}}/{{y}}"], 6))
        static = bundle["files"]["routing/packs/" + "c" * 64 + "/pages.bin"]["transport"]["sha256"]
        self.assertFalse((directory / "objects" / static).exists())
        self.assertEqual(before, {p.name: p.read_bytes() for p in (self.source / "objects").iterdir()})
        restarted = downloads.Downloads(self.source, self.service.cache, 1000000)
        self.assertEqual(first, restarted.prepare(request))

    def test_new_selection_has_no_reservation_and_is_immediate(self):
        request = self.request()
        first = self.service.prepare(request)
        second = self.service.prepare(self.request([7.1, 47.1, 7.9, 48]))
        self.assertEqual(first["state"], "ready")
        self.assertEqual(second["state"], "ready")
        self.assertNotEqual(first["id"], second["id"])
        self.assertIsNotNone(self.service.selection(first["id"]))

    def test_invalid_coverage_and_disk_capacity_do_not_leave_selections(self):
        for value in ([0, 0, 1, 1], [7, 47, 7, 48], [7, 47, float("nan"), 48], [True, 47, 8, 48]):
            with self.subTest(bounds=value), self.assertRaises(ValueError):
                self.service.prepare(self.request(value))
        self.service.max_cache_bytes = 1
        with self.assertRaisesRegex(ValueError, "no space"):
            self.service.prepare(self.request())
        self.assertFalse(list(self.service.cache.iterdir()))

    def test_failed_metadata_write_cleans_staging_and_allows_retry(self):
        original = Path.write_bytes
        def write(path, data):
            if path.name == "bundle.json": raise OSError("disk write failed")
            return original(path, data)
        with patch.object(Path, "write_bytes", write), self.assertRaises(OSError):
            self.service.prepare(self.request())
        self.assertFalse(list(self.service.cache.iterdir()))
        self.assertEqual(self.service.prepare(self.request())["state"], "ready")

    def test_publication_identity_covers_packing_changes_and_checks_cell_metadata(self):
        first = self.service.prepare(self.request())
        path = self.source / "catalog.json"
        publication = json.loads(path.read_bytes())
        publication["map_zoom"] = 12
        path.write_bytes(runtime.encoded(publication))
        newer = downloads.Downloads(self.source, self.service.cache, 1000000)
        self.assertNotEqual(first["id"], newer.prepare(self.request())["id"])
        (self.source / "routing-cells/left.json").write_bytes(b"{}")
        with self.assertRaisesRegex(ValueError, "checksum"):
            newer.prepare(self.request([7.1,47.1,7.9,48]))

    def test_metadata_cache_evicts_the_oldest_selection_and_keeps_origin(self):
        first = self.service.prepare(self.request([7.1,47.1,7.9,48]))
        directory = self.service.cache / first["id"]
        origin = json.loads((directory / "origin.json").read_bytes())
        self.assertEqual(origin, {"source": str(self.source.resolve()), "objects_url": None})
        second = self.service.prepare(self.request([8.1,47.1,8.9,48]))
        third = self.service.prepare(self.request())
        size = lambda identity: sum(p.stat().st_size for p in (self.service.cache / identity).rglob("*") if p.is_file())
        self.service.max_cache_bytes = size(second["id"]) + size(third["id"])
        import shutil
        shutil.rmtree(self.service.cache / third["id"])
        os.utime(directory, (1, 1))
        self.assertEqual(third, self.service.prepare(self.request()))
        self.assertIsNone(self.service.selection(first["id"]))
        self.assertIsNotNone(self.service.selection(second["id"]))

    def serve(self):
        server = downloads.ThreadingHTTPServer(("127.0.0.1", 0), downloads.handler(self.service))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        def close():
            server.shutdown(); server.server_close(); thread.join()
        self.addCleanup(close)
        return f"http://127.0.0.1:{server.server_port}"

    def test_generated_metadata_travels_gzip_and_decodes_to_the_release_file(self):
        job = self.service.prepare(self.request())
        base = f"{self.serve()}/bundles/{job['id']}"
        with urlopen(f"{base}/bundle.json") as response:
            entry = json.load(response)["files"]["routing/blocks.json"]
        self.assertEqual(entry["transport"]["encoding"], "gzip")
        with urlopen(f"{base}/objects/{entry['transport']['sha256']}") as response:
            data = gzip.decompress(response.read())
        self.assertEqual((len(data), hashlib.sha256(data).hexdigest()), (entry["bytes"], entry["sha256"]))
        self.assertEqual(json.loads(data)["archives"], ["c" * 64])

    def test_http_supports_exact_ranges_and_rejects_traversal(self):
        job = self.service.prepare(self.request())
        base = self.serve()
        bundle = json.loads((self.service.cache / job["id"] / "bundle.json").read_bytes())
        digest = bundle["files"]["routing/packs/" + "c" * 64 + "/pages.bin"]["transport"]["sha256"]
        url = f"{base}/bundles/{job['id']}/objects/{digest}"
        # A selection being built holds the cache lock; object reads never wait for it.
        with self.service.lock, urlopen(Request(url, headers={"Range": "bytes=2-4"}), timeout=5) as response:
            self.assertEqual(response.status, 206)
            self.assertEqual(response.headers["Content-Range"], "bytes 2-4/6")
            self.assertEqual(response.read(), b"cde")
        with urlopen(Request(url, method="HEAD")) as response:
            self.assertEqual(response.headers["Content-Length"], "6")
            self.assertEqual(response.read(), b"")
        with self.assertRaises(HTTPError) as failure:
            urlopen(Request(url, headers={"Range": "bytes=9-"}))
        self.assertEqual(failure.exception.code, 416)
        with self.assertRaises(HTTPError) as failure:
            urlopen(f"{base}/bundles/{job['id']}/%2e%2e/release.json")
        self.assertEqual(failure.exception.code, 404)
