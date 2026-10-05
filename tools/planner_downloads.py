"""Select published map blocks and serve their immutable resumable objects."""

import argparse
import gzip
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import re
import shutil
import threading
from urllib.parse import unquote, urlsplit

from . import planner_runtime, planner_blocks, planner_maps, planner_grid, planner_map_archive


def contains(outer, inner):
    return outer[0] <= inner[0] < inner[2] <= outer[2] and outer[1] <= inner[1] < inner[3] <= outer[3]


def bounds(value):
    if (not isinstance(value, list) or len(value) != 4
            or any(type(x) not in (int, float) or not math.isfinite(x) for x in value)
            or not contains([-180, -85, 180, 85], value)):
        raise ValueError("Choose a valid map area.")
    return value


class Downloads:
    def __init__(self, source, cache, max_cache_bytes, objects_url=None):
        self.source, self.cache = source.resolve(), cache.resolve()
        self.publication = json.loads((self.source / "catalog.json").read_bytes())
        if self.publication["format"] != 3:
            raise ValueError("Unsupported offline publication")
        self.identity, self.manifest = planner_runtime.digest(self.source / "catalog.json"), self.publication["release"]
        self.objects_url = objects_url.rstrip("/") if objects_url else None
        self.max_cache_bytes = max_cache_bytes
        self.cache.mkdir(parents=True, exist_ok=True)
        self.lock = threading.Lock()

    def prepare(self, request):
        if set(request) != {"bounds"}:
            raise ValueError("Choose one map area.")
        selection = bounds(request["bounds"])
        if not contains(self.manifest["bounds"], selection):
            raise ValueError("Choose an area inside the available planner coverage.")
        cells = [c for c in self.publication["cells"] if self.overlaps(c["bounds"], selection)]
        if not cells: raise ValueError("This area has no published map data.")
        actual = [min(c["bounds"][0] for c in cells), min(c["bounds"][1] for c in cells),
                  max(c["bounds"][2] for c in cells), max(c["bounds"][3] for c in cells)]
        identity = hashlib.sha256(planner_runtime.encoded({"format": 3, "source": self.identity, "bounds": actual})).hexdigest()
        with self.lock:
            destination = self.cache / identity
            if not (destination / "bundle.json").exists():
                self.quote(destination, actual, cells, identity)
        return {"id": identity, "state": "ready", "progress": 1}

    @staticmethod
    def overlaps(a, b):
        return a[0] < b[2] and a[2] > b[0] and a[1] < b[3] and a[3] > b[1]

    def quote(self, destination, actual, cells, identity):
        publication = self.publication
        selected = [c for c in cells if c.get("routing")]
        selections = []
        for cell in selected:
            path = self.source / cell["routing"]["manifest"]
            if not path.resolve().is_relative_to(self.source) or planner_runtime.digest(path) != cell["routing"]["sha256"]:
                raise ValueError("Routing cell checksum mismatch")
            selections.append(json.loads(path.read_bytes()))
        graph = planner_grid.routing(selections, [c["routing"]["adjacency"] for c in selected], "area-" + identity[:24], actual)
        shared = publication["shared"]
        names = set(shared)
        geometry = [min([actual[i], *[c["routing"]["geometry_bounds"][i] for c in selected]]) if i < 2
                    else max([actual[i], *[c["routing"]["geometry_bounds"][i] for c in selected]]) for i in range(4)]
        terrain = planner_maps.terrain_bounds(geometry)
        for b in publication["map_blocks"]:
            if planner_map_archive.selected(b["tile"], geometry, terrain=b["kind"] == "terrain"): names.update(b["files"])
        for cell in cells: names.update(cell["files"])
        for archive in graph["archives"]:
            names.update(f"routing/packs/{archive}/{filename}" for filename in ("pages.bin", "pages.idx"))
        files = {name: publication["files"][shared.get(name, name)] for name in sorted(names)}
        generated = {}
        def metadata(name, document):
            data = planner_runtime.encoded(document)
            wire = gzip.compress(data, compresslevel=6, mtime=0)
            transport = {"bytes": len(wire), "sha256": hashlib.sha256(wire).hexdigest(), "encoding": "gzip"}
            generated[transport["sha256"]] = wire
            files[name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "transport": transport}
            return files[name]["sha256"]
        package = metadata("routing/blocks.json", graph)
        metadata("routing/layers.json", [c["id"] for c in cells])
        for kind in ("basemap", "places", "terrain"):
            metadata(f"maps/{kind}.json", {"tilejson": "3.0.0", "tiles": [
                f"https://offline.openbikecomputer.invalid/{identity}/{kind}/{{z}}/{{x}}/{{y}}"],
                "minzoom": 11 if kind == "places" else 0, "maxzoom": {"basemap": 14, "places": 11, "terrain": 12}[kind],
                "bounds": terrain if kind == "terrain" else geometry})
        release = {**self.manifest, "format": 1, "region": graph["data"]["region"], "bounds": actual,
                   "routing_package": package, "source_files": {}, "terrain_bounds": terrain,
                   "offline": {"format": 2, "id": identity, "zoom": publication["zoom"], "map_zoom": publication["map_zoom"],
                               "source_routing": publication["routing_source"],
                               "cells": [{"id": c["id"], "bounds": c["bounds"]} for c in cells]},
                   "files": {name: {k: item[k] for k in ("bytes", "sha256")} for name, item in files.items()}}
        encoded = planner_runtime.encoded(release)
        bundle = planner_runtime.encoded({"format": 1, "release": {"bytes": len(encoded),
            "sha256": hashlib.sha256(encoded).hexdigest()}, "files": files})
        origin = planner_runtime.encoded({"source": str(self.source), "objects_url": self.objects_url})
        needed = len(encoded) + len(bundle) + len(origin) + sum(map(len, generated.values()))
        if needed > self.max_cache_bytes or shutil.disk_usage(self.cache).free < needed:
            raise ValueError("The download service has no space for a new selection.")
        entries = sorted((p for p in self.cache.iterdir() if p.is_dir() and re.fullmatch(r"[0-9a-f]{64}", p.name)),
                         key=lambda p: p.stat().st_mtime)
        sizes = {p: sum(f.stat().st_size for f in p.rglob("*") if f.is_file()) for p in entries}
        used = sum(sizes.values())
        for path in entries:
            if used + needed <= self.max_cache_bytes: break
            shutil.rmtree(path)
            used -= sizes[path]
        temporary = self.cache / ("." + destination.name + ".work")
        shutil.rmtree(temporary, ignore_errors=True)
        try:
            (temporary / "objects").mkdir(parents=True)
            for sha, data in generated.items(): (temporary / "objects" / sha).write_bytes(data)
            (temporary / "release.json").write_bytes(encoded)
            (temporary / "bundle.json").write_bytes(bundle)
            (temporary / "origin.json").write_bytes(origin)
            temporary.rename(destination)
        finally:
            shutil.rmtree(temporary, ignore_errors=True)

    def status(self, identity):
        if not re.fullmatch(r"[0-9a-f]{64}", identity): raise ValueError("Invalid download.")
        directory = self.cache / identity
        ready = (directory / "bundle.json").exists()
        if ready:
            try: os.utime(directory, None)
            except FileNotFoundError: ready = False
        return {"id": identity, "state": "ready" if ready else "failed"}


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
                return self.json({"format": 1, "bounds": downloads.manifest["bounds"],
                                  "zoom": downloads.publication["zoom"]})
            if re.fullmatch(r"/jobs/[0-9a-f]{64}", path):
                return self.json(downloads.status(path.split("/")[-1]))
            match = re.fullmatch(r"/bundles/([0-9a-f]{64})/(bundle.json|release.json|objects/[0-9a-f]{64})", path)
            with downloads.lock:
                if not match or downloads.status(match[1])["state"] != "ready":
                    return self.json({"message": "Download not found. Prepare the map again."}, 404)
                file = downloads.cache / match[1] / match[2]
                if not file.is_file() and match[2].startswith("objects/"):
                    origin = json.loads((downloads.cache / match[1] / "origin.json").read_bytes())
                    if origin["objects_url"]:
                        self.send_response(307)
                        self.send_header("Location", origin["objects_url"] + "/" + file.name)
                        self.send_header("Content-Length", "0")
                        self.end_headers()
                        return
                    file = Path(origin["source"]) / match[2]
                try:
                    stream = file.open("rb")
                except FileNotFoundError:
                    return self.json({"message": "File not found."}, 404)
            # An open descriptor remains valid if the selection is evicted.
            with stream:
                self.send_file(stream, match[1], file.name)

        def send_file(self, stream, identity, name):
            size = os.fstat(stream.fileno()).st_size
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
            self.send_header("ETag", '"' + identity + '-' + name + '"')
            self.send_header("Content-Length", str(end - start + 1))
            if status == 206:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.end_headers()
            if self.command != "HEAD":
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
    parser.add_argument("--port", type=int, default=8790)
    parser.add_argument("--objects-url", help="Public URL of the release's immutable object pool")
    args = parser.parse_args()
    if args.max_cache_bytes <= 0:
        parser.error("--max-cache-bytes must be positive")
    downloads = Downloads(args.source, args.cache, args.max_cache_bytes, args.objects_url)
    ThreadingHTTPServer(("127.0.0.1", args.port), handler(downloads)).serve_forever()


if __name__ == "__main__":
    main()
