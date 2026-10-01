"""Serve bounded, cached offline planner preparations and resumable bundle objects."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import logging
import math
from pathlib import Path
import re
import shutil
import threading
import tomllib
from urllib.parse import unquote, urlsplit

from . import planner_cutout, planner_offline, planner_runtime


def contains(outer, inner):
    return outer[0] <= inner[0] < inner[2] <= outer[2] and outer[1] <= inner[1] < inner[3] <= outer[3]


def bounds(value):
    if (not isinstance(value, list) or len(value) != 4
            or any(type(x) not in (int, float) or not math.isfinite(x) for x in value)
            or not contains([-180, -85, 180, 85], value)):
        raise ValueError("Choose a valid map area.")
    return value


class Downloads:
    def __init__(self, source, cache, max_cache_bytes, regions=None):
        self.source, self.cache = source.resolve(), cache.resolve()
        self.identity, self.manifest = planner_runtime.release(source, include_sources=False)
        self.max_cache_bytes = max_cache_bytes
        self.cache.mkdir(parents=True, exist_ok=True)
        self.executor = ThreadPoolExecutor(max_workers=1)
        self.lock = threading.Lock()
        self.jobs = {}
        self.regions = self.catalog(regions)

    def catalog(self, index=None):
        source_bounds = bounds(self.manifest["bounds"])
        path = self.source / "device/catalog.json"
        regions = []
        if index:
            configured = tomllib.loads((Path(__file__).resolve().parents[1] / "host/obc-bake/regions.toml").read_text())["regions"]
            names = {r["id"].split("/")[-1]: r["name"] for r in configured}
            for feature in json.loads(index.read_bytes())["features"]:
                props, geometry = feature["properties"], feature["geometry"]
                if props["id"] not in names:
                    continue
                polygons = geometry["coordinates"] if geometry["type"] == "MultiPolygon" else [geometry["coordinates"]]
                regions.append({"id": props["id"], "name": names[props["id"]], "parent": props.get("parent"),
                                "rings": [ring for polygon in polygons for ring in polygon]})
        elif path.exists():
            for region in json.loads(path.read_bytes())["regions"]:
                rings = [[[point[1] / 1e6, point[0] / 1e6] for point in ring] for ring in region["boundary"]["rings"]]
                regions.append({"id": region["id"], "name": region["name"], "parent": region.get("parent"), "rings": rings})
        for region in regions:
            points = [p for ring in region["rings"] for p in ring]
            box = bounds([min(p[0] for p in points), min(p[1] for p in points),
                          max(p[0] for p in points), max(p[1] for p in points)])
            region.update(bounds=box, available=contains(source_bounds, box))
        # The source's declared rectangle is the available planner coverage.
        west, south, east, north = source_bounds
        regions.append({"id": "coverage", "name": "Available map coverage",
                        "parent": None, "bounds": source_bounds, "available": True,
                        "rings": [[[west, south], [east, south], [east, north], [west, north], [west, south]]]})
        return list({region["id"]: region for region in regions}.values())

    def prepare(self, request):
        if set(request) == {"region"}:
            region = next((r for r in self.regions if r["id"] == request["region"]), None)
            if not region or not region["available"]:
                raise ValueError("This region is not available for offline planning.")
            selection = region["bounds"]
        elif set(request) == {"bounds"}:
            selection = bounds(request["bounds"])
        else:
            raise ValueError("Choose one region or one map area.")
        if not contains(self.manifest["bounds"], selection):
            raise ValueError("Choose an area inside the available planner coverage.")
        identity = hashlib.sha256(planner_runtime.encoded({"source": self.identity, "bounds": selection})).hexdigest()
        with self.lock:
            if (self.cache / identity / "bundle.json").exists():
                return {"id": identity, "state": "ready"}
            if identity in self.jobs and self.jobs[identity]["state"] != "failed":
                return dict(self.jobs[identity])
            if any(job["state"] == "preparing" for job in self.jobs.values()):
                raise ValueError("Another map is being prepared. Try again in a moment.")
            used = sum(p.stat().st_size for p in self.cache.rglob("*") if p.is_file())
            runtime = sum(f["bytes"] for f in self.manifest["files"].values())
            # A cutout and its transport can coexist. Reserve the full source size for each.
            if used + runtime * 2 > self.max_cache_bytes or shutil.disk_usage(self.cache).free < runtime * 2:
                raise ValueError("The download service has no space for another map. Try again later.")
            job = {"id": identity, "state": "preparing"}
            self.jobs[identity] = job
            self.executor.submit(self.build, identity, selection)
            return dict(job)

    def build(self, identity, selection):
        release = self.cache / (identity + ".release")
        stage = self.cache / (identity + ".part")
        try:
            for directory in (release, stage):
                if directory.exists():
                    shutil.rmtree(directory)
            source = self.source
            if selection != self.manifest["bounds"]:
                planner_cutout.prepare(source, release, selection, "area-" + identity[:24])
                source = release
            planner_offline.pack(source, stage)
            stage.rename(self.cache / identity)
            with self.lock:
                self.jobs[identity] = {"id": identity, "state": "ready"}
        except Exception:
            logging.exception("Offline preparation failed: %s", identity)
            with self.lock:
                self.jobs[identity] = {"id": identity, "state": "failed", "message": "The map could not be prepared. Try a smaller area."}
        finally:
            for directory in (release, stage):
                if directory.exists():
                    shutil.rmtree(directory)

    def status(self, identity):
        if not re.fullmatch(r"[0-9a-f]{64}", identity):
            raise ValueError("Invalid download.")
        with self.lock:
            if (self.cache / identity / "bundle.json").exists():
                return {"id": identity, "state": "ready"}
            return dict(self.jobs.get(identity, {"id": identity, "state": "failed", "message": "Prepare this map again."}))


def handler(downloads):
    class Handler(BaseHTTPRequestHandler):
        def json(self, value, status=200):
            data = planner_runtime.encoded(value)
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(data)

        def do_POST(self):
            try:
                if self.path != "/jobs":
                    return self.json({"message": "Not found."}, 404)
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 4096:
                    return self.json({"message": "Invalid selection."}, 400)
                request = json.loads(self.rfile.read(size))
                if not isinstance(request, dict):
                    raise ValueError("Invalid selection.")
                self.json(downloads.prepare(request), 202)
            except (ValueError, TypeError) as error:
                self.json({"message": str(error)}, 400)

        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            path = unquote(urlsplit(self.path).path)
            if path == "/catalog":
                return self.json({"format": 1, "bounds": downloads.manifest["bounds"], "regions": downloads.regions})
            if re.fullmatch(r"/jobs/[0-9a-f]{64}", path):
                return self.json(downloads.status(path.split("/")[-1]))
            match = re.fullmatch(r"/bundles/([0-9a-f]{64})/(bundle.json|release.json|objects/[0-9a-f]{64})", path)
            if not match or downloads.status(match[1])["state"] != "ready":
                return self.json({"message": "Download not found. Prepare the map again."}, 404)
            file = downloads.cache / match[1] / match[2]
            if not file.is_file():
                return self.json({"message": "File not found."}, 404)
            size = file.stat().st_size
            start, end, status = 0, size - 1, 200
            if "Range" in self.headers:
                byte_range = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers["Range"])
                if byte_range:
                    start = int(byte_range[1])
                    end = min(int(byte_range[2]) if byte_range[2] else size - 1, size - 1)
                if not byte_range or not 0 <= start <= end < size:
                    self.send_response(416)
                    self.send_header("Content-Range", f"bytes */{size}")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                status = 206
            self.send_response(status)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", '"' + match[1] + '-' + file.name + '"')
            self.send_header("Content-Length", str(end - start + 1))
            if status == 206:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.end_headers()
            if self.command != "HEAD":
                with file.open("rb") as stream:
                    stream.seek(start)
                    remaining = end - start + 1
                    while remaining:
                        chunk = stream.read(min(remaining, 1024 * 1024))
                        if not chunk:
                            break
                        self.wfile.write(chunk)
                        remaining -= len(chunk)
    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--max-cache-bytes", type=int, required=True)
    parser.add_argument("--regions", type=Path, help="Geofabrik index-v1.json with boundary geometry")
    parser.add_argument("--port", type=int, default=8790)
    args = parser.parse_args()
    if args.max_cache_bytes <= 0:
        parser.error("--max-cache-bytes must be positive")
    downloads = Downloads(args.source, args.cache, args.max_cache_bytes, args.regions)
    ThreadingHTTPServer(("127.0.0.1", args.port), handler(downloads)).serve_forever()


if __name__ == "__main__":
    main()
