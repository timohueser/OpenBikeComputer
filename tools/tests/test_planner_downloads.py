"""Download jobs preserve coverage, reuse complete bundles, and serve resumable bytes."""

from concurrent.futures import Future
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from tools import planner_downloads as downloads, planner_runtime as runtime
from tools.tests.test_planner_offline import release


class DeferredExecutor:
    def submit(self, callback, *args):
        self.work = (callback, args)
        return Future()


class PlannerDownloads(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        release(self.source, {"routing/data.bin": b"abcdef"})
        manifest = json.loads((self.source / "release.json").read_bytes())
        manifest["bounds"] = [7, 47, 9, 49]
        (self.source / "release.json").write_bytes(runtime.encoded(manifest))
        with patch.object(downloads, "ThreadPoolExecutor", return_value=DeferredExecutor()):
            self.service = downloads.Downloads(self.source, self.root / "cache", 1000000)

    def test_serial_jobs_reuse_complete_bundles_across_restart(self):
        selection = {"bounds": [7, 47, 9, 49]}
        job = self.service.prepare(selection)
        self.assertEqual(job, self.service.prepare(selection))
        with self.assertRaisesRegex(ValueError, "Another map"):
            self.service.prepare({"bounds": [7.1, 47.1, 8, 48]})
        callback, args = self.service.executor.work
        callback(*args)
        self.assertEqual(self.service.status(job["id"])["state"], "ready")
        self.service.jobs.clear()
        self.assertEqual(self.service.prepare(selection)["state"], "ready")
        self.assertFalse(list(self.service.cache.glob("*.part")))
        self.assertFalse(list(self.service.cache.glob("*.release")))

    def test_invalid_coverage_and_disk_capacity_do_not_start_work(self):
        for value in ([0, 0, 1, 1], [7, 47, 7, 48], [7, 47, float("nan"), 48], [True, 47, 8, 48]):
            with self.subTest(bounds=value), self.assertRaises(ValueError):
                self.service.prepare({"bounds": value})
        self.service.max_cache_bytes = 1
        with self.assertRaisesRegex(ValueError, "no space"):
            self.service.prepare({"bounds": [7, 47, 9, 49]})
        self.assertFalse(self.service.jobs)

    def test_failed_preparation_is_retryable_and_cleans_staging(self):
        job = self.service.prepare({"bounds": [7.1, 47.1, 8, 48]})
        callback, args = self.service.executor.work
        with patch.object(downloads.planner_cutout, "prepare", side_effect=ValueError("failed")), self.assertLogs(level="ERROR"):
            callback(*args)
        self.assertEqual(self.service.status(job["id"])["state"], "failed")
        self.assertEqual(self.service.prepare({"bounds": [7.1, 47.1, 8, 48]})["state"], "preparing")
        self.assertFalse(list(self.service.cache.iterdir()))

    def test_catalog_keeps_nested_region_choices_and_marks_uncovered_regions(self):
        index = self.root / "regions.json"
        def feature(identity, parent, box):
            w, s, e, n = box
            return {"properties": {"id": identity, "name": identity, "parent": parent},
                    "geometry": {"type": "MultiPolygon", "coordinates": [[[[w,s],[e,s],[e,n],[w,n],[w,s]]]]}}
        index.write_bytes(runtime.encoded({"features": [
            feature("germany", "europe", [5,45,15,55]),
            feature("baden-wuerttemberg", "germany", [7,47,9,49]),
            feature("freiburg-regbez", "baden-wuerttemberg", [7.1,47.1,8,48])]}))
        regions = {r["id"]: r for r in self.service.catalog(index)}
        self.assertFalse(regions["germany"]["available"])
        self.assertTrue(regions["freiburg-regbez"]["available"])
        self.assertEqual(regions["freiburg-regbez"]["parent"], "baden-wuerttemberg")
        self.assertEqual(regions["baden-wuerttemberg"]["name"], "Baden-Württemberg")

    def test_http_supports_exact_ranges_and_rejects_traversal(self):
        job = self.service.prepare({"bounds": [7,47,9,49]})
        callback, args = self.service.executor.work
        callback(*args)
        server = downloads.ThreadingHTTPServer(("127.0.0.1", 0), downloads.handler(self.service))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        def close():
            server.shutdown(); server.server_close(); thread.join()
        self.addCleanup(close)
        base = f"http://127.0.0.1:{server.server_port}"
        bundle = json.loads((self.service.cache / job["id"] / "bundle.json").read_bytes())
        digest = bundle["files"]["routing/data.bin"]["transport"]["sha256"]
        url = f"{base}/bundles/{job['id']}/objects/{digest}"
        with urlopen(Request(url, headers={"Range": "bytes=2-4"})) as response:
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
