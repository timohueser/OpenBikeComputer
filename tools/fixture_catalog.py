#!/usr/bin/env python3
"""Digest-pinned catalog fixtures on loopback, for the two journeys that drive a real catalog UI.

Two document sets, one server. `schema_examples()` is the desktop launch smoke's catalog and
publishes no cell bytes, because that journey stops at region selection. `web_assemble()` is the
builder browser journey's catalog, where every artifact is real, so a region download from it
assembles the checked-in cells into the checked-in map.

Every `bytes`, `sha256` and `url` is computed from the bytes this module serves, never copied from
a document.

Run it as a server for a browser test:

    python3 tools/fixture_catalog.py --catalog web-assemble --port 4180 \\
        --static builder/app/dist/web --log .artifacts/web-builder/catalog.jsonl
"""

from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
from threading import Thread
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_EXAMPLES = ROOT / "host/obc-pack/schema"
ASSEMBLE_FIXTURE = ROOT / "apps/obc-web-assemble/tests/fixture"

CONTENT_TYPES = {
    ".json": "application/json",
    ".obcm": "application/octet-stream",
    ".obcd": "application/octet-stream",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".wasm": "application/wasm",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".ico": "image/vnd.microsoft.icon",
    ".txt": "text/plain; charset=utf-8",
}


def content_type(path: str) -> str:
    return CONTENT_TYPES.get(Path(path).suffix, "application/octet-stream")


def pin(objects: dict[str, bytes], path: str, body: bytes) -> dict:
    """Publish `body` at `path` with its digest before the extension, and return the reference a
    document carries for it (`OBCC_Spec.md` §9)."""

    digest = hashlib.sha256(body).hexdigest()
    stem, _, extension = path.rpartition(".")
    url = f"{stem}.{digest}.{extension}"
    objects[url] = body
    return {"url": url, "bytes": len(body), "sha256": digest}


def _document(objects: dict[str, bytes], path: str, document: object) -> dict:
    return pin(objects, path, json.dumps(document).encode())


# The desktop launch smoke's catalog.


def schema_examples() -> dict[str, bytes]:
    """Keep the producer's fine-band example; make the other bands empty."""

    catalog = json.loads((SCHEMA_EXAMPLES / "catalog.example.json").read_text())
    fine = json.loads((SCHEMA_EXAMPLES / "cell-index.example.json").read_text())
    region_cells = json.loads((SCHEMA_EXAMPLES / "region-cells.example.json").read_text())
    objects: dict[str, bytes] = {}

    catalog.pop("terrain", None)
    catalog.pop("network_terrain_revision", None)
    for ref in catalog["cell_index"]:
        doc = fine if ref["band"] == "fine" else {
            "schema_version": 3, "schema_revision": catalog["schema"]["revision"],
            "band": ref["band"], "cells": [], "known_empty": [],
        }
        ref.update(_document(objects, f"/{ref['band']}.json", doc))
        ref.update(cell_count=len(doc["cells"]), known_empty_count=len(doc["known_empty"]))
    region_cells["cells"] = {"fine": region_cells["cells"]["fine"]}
    region_cells.pop("terrain", None)
    region = catalog["regions"][0]
    region.pop("terrain", None)
    region.update(bytes=sum(cell["bytes"] for cell in fine["cells"]),
                  bytes_by_band={"fine": sum(cell["bytes"] for cell in fine["cells"])}, cell_count={"fine": 3},
                  partial_cell_count_by_band={"fine": 0})
    pinned = _document(objects, "/region.json", region_cells)
    region.update({f"cells_{key}": value for key, value in pinned.items()})
    catalog["regions"] = [region]
    objects["/catalog.json"] = json.dumps(catalog).encode()
    return objects


# The builder browser journey's catalog.
#
# The bridge fixture's own cut numbers its feature types in config order. A hosted catalog names the
# feature types and the schema assigns the ids, so the assignment is restated here in the catalog's
# own spelling. The ids reach the cells' chunk bytes; the names are what the skin resolves against.
FEATURE_TYPES = {
    1: "natural.water",
    2: "highway.primary",
    3: "highway.residential",
    4: "highway.path",
}

# Where this catalog is published on the server, and what the browser suite builds
# `VITE_CATALOG_URL` from.
PREFIX = "/catalog"
REGION_ID = "bridge-fixture"
REGION_NAME = "Bridge Fixture"
SCHEMA_REVISION = 1
TERRAIN_REVISION = 1
DATASET_ID = "fixture-raster"
DATASET_VERSION = "1"
BUILT_AT = "2026-07-30T02:12:55Z"
GENERATED_AT = "2026-07-30T09:00:00Z"
SOURCES = [{"extract_id": "fixture", "snapshot": "2026-07-19"}]


def _cell_path(band: str, cell_id: str, extension: str) -> str:
    _log2, i, j = cell_id.split("/")
    return f"{PREFIX}/cells/{band}/{i}/{j}.{extension}"


def web_assemble() -> dict[str, bytes]:
    """The bridge fixture published as a catalog: schema, skin, cells, terrain, one region."""

    sidecar = json.loads((ASSEMBLE_FIXTURE / "cells.json").read_text())
    skin_doc = json.loads((ASSEMBLE_FIXTURE / "skin.json").read_text())
    terrain_doc = json.loads((ASSEMBLE_FIXTURE / "terrain.json").read_text())
    objects: dict[str, bytes] = {}

    styles = sorted(skin_doc["styles"], key=lambda style: style["id"])

    # The cut's own schema, verbatim, plus the three things a catalog root states that a local
    # sidecar does not: an identity, the canonical style-id assignment, and the routing profiles.
    schema = dict(sidecar["schema"])
    schema.update(
        id="bridge-fixture",
        revision=SCHEMA_REVISION,
        name="Bridge Fixture",
        description="The obc-web-assemble bridge fixture's cut, published as a catalog schema.",
        styles=[{"id": s["id"], "feature_type": FEATURE_TYPES[s["id"]]} for s in styles],
    )
    schema["routing"] = {**schema.get("routing", {}), "profiles": []}

    skin = {
        "id": "fixture",
        "name": skin_doc["name"],
        "description": "The cut's own styling, which is what the expected map was stamped with.",
        "version": 1,
        "marker_color": skin_doc["marker_color"],
        "styles": [
            {
                "feature_type": FEATURE_TYPES[s["id"]],
                "color": s["color"], "weight": s["weight"], "z_index": s["z_index"],
                "priority": s["priority"], "line_style": s["line_style"],
                "fixed_width": s["fixed_width"], "terrain_layer": s["terrain_layer"],
                "color2": s["color2"],
            }
            for s in styles
        ],
    }

    # One cell index per band, each cell pinned by the bytes this server hands over.
    by_band: dict[str, list[dict]] = {band["id"]: [] for band in schema["bands"]}
    for cell in sidecar["cells"]:
        body = (ASSEMBLE_FIXTURE / cell["path"]).read_bytes()
        entry = {"id": cell["id"], "built_at": BUILT_AT, "sources": SOURCES, "partial": cell["partial"]}
        entry.update(pin(objects, _cell_path(cell["band"], cell["id"], "obcm"), body))
        by_band[cell["band"]].append(entry)

    cell_index = []
    for band in sorted(schema["bands"], key=lambda b: -b["cell_log2"]):
        cells = by_band[band["id"]]
        document = {
            "schema_version": 3, "schema_revision": SCHEMA_REVISION, "band": band["id"],
            "cells": cells, "known_empty": [],
        }
        ref = {"band": band["id"], "cell_log2": band["cell_log2"],
               "cell_count": len(cells), "known_empty_count": 0}
        ref.update(_document(objects, f"{PREFIX}/cells/{band['id']}/index.json", document))
        cell_index.append(ref)

    terrain_cells = []
    for cell in terrain_doc["cells"]:
        body = (ASSEMBLE_FIXTURE / cell["path"]).read_bytes()
        entry = {"id": cell["id"], "built_at": BUILT_AT}
        entry.update(pin(objects, _cell_path("terrain", cell["id"], "obcd"), body))
        terrain_cells.append(entry)
    terrain_index = {
        "schema_version": 3, "terrain_revision": TERRAIN_REVISION,
        "dataset_id": DATASET_ID, "dataset_version": DATASET_VERSION,
        "posting_log2": terrain_doc["posting_log2"], "cell_log2": terrain_doc["cell_log2"],
        "cells": terrain_cells, "known_empty": [],
    }
    terrain = {
        "dataset_id": DATASET_ID, "dataset_version": DATASET_VERSION,
        "posting_log2": terrain_doc["posting_log2"], "cell_log2": terrain_doc["cell_log2"],
        "terrain_revision": TERRAIN_REVISION,
        "attribution": "Synthetic raster cut by apps/obc-web-assemble/examples/fixture.rs.",
        "cell_index": {"cell_count": len(terrain_cells), "known_empty_count": 0,
                       **_document(objects, f"{PREFIX}/cells/terrain/index.json", terrain_index)},
    }

    region_cells = {
        "schema_version": 3, "schema_revision": SCHEMA_REVISION, "region_id": REGION_ID,
        "cells": {band: [cell["id"] for cell in cells] for band, cells in by_band.items() if cells},
        "terrain": [cell["id"] for cell in terrain_cells],
    }
    bytes_by_band = {band: sum(cell["bytes"] for cell in cells) for band, cells in by_band.items()}
    lat_min, lon_min, lat_max, lon_max = sidecar["extract_bbox"]
    region = {
        "id": REGION_ID,
        "name": REGION_NAME,
        "boundary": {
            "tolerance_udeg": 0,
            "rings": [[[lat_min, lon_min], [lat_max, lon_min], [lat_max, lon_max],
                       [lat_min, lon_max], [lat_min, lon_min]]],
        },
        "bytes": sum(bytes_by_band.values()),
        "bytes_by_band": bytes_by_band,
        "cell_count": {band: len(cells) for band, cells in by_band.items()},
        "partial_cell_count_by_band": {
            band: sum(1 for cell in cells if cell["partial"]) for band, cells in by_band.items()
        },
        "terrain": {"cell_count": len(terrain_cells), "known_empty_count": 0,
                    "bytes": sum(cell["bytes"] for cell in terrain_cells)},
    }
    pinned = _document(objects, f"{PREFIX}/regions/{REGION_ID}/cells.json", region_cells)
    region.update({f"cells_{key}": value for key, value in pinned.items()})

    catalog = {
        "schema_version": 3,
        "generated_at": GENERATED_AT,
        "source": {
            "dataset_id": "fixture",
            "attribution": "Synthetic geometry cut by apps/obc-web-assemble/examples/fixture.rs.",
            "license": "CC0-1.0",
            "license_url": "https://creativecommons.org/publicdomain/zero/1.0/",
        },
        "schema": schema,
        "skins": [skin],
        "regions": [region],
        "cell_index": cell_index,
        "terrain": terrain,
        "network_terrain_revision": TERRAIN_REVISION,
    }
    objects[f"{PREFIX}/catalog.json"] = json.dumps(catalog).encode()
    return objects


CATALOGS = {"schema-examples": schema_examples, "web-assemble": web_assemble}




class CatalogServer(ThreadingHTTPServer):
    """Serves a pinned object set, and optionally a static build alongside it on one origin.

    One origin matters: the builder fetches cells with `fetch`, and a second port would be a
    cross-origin request needing CORS that the real deployment does not need.
    """

    daemon_threads = True

    def __init__(self, objects: dict[str, bytes], port: int = 0,
                 static: Path | None = None, log: Path | None = None) -> None:
        super().__init__(("127.0.0.1", port), _Handler)
        self.objects = objects
        self.static = static.resolve() if static else None
        self.requests: list[str] = []
        self.log = log
        if log:
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text(json.dumps({"served": sorted(objects)}) + "\n")

    @property
    def origin(self) -> str:
        return f"http://127.0.0.1:{self.server_port}"

    def start(self) -> "CatalogServer":
        Thread(target=self.serve_forever, daemon=True).start()
        return self

    def record(self, path: str, status: int, kind: str) -> None:
        self.requests.append(path)
        if self.log:
            with self.log.open("a") as handle:
                handle.write(json.dumps({"path": path, "status": status, "kind": kind}) + "\n")

    def static_file(self, path: str) -> Path | None:
        """A file under the static root, or `None`. A path that escapes the root is not one."""

        if not self.static:
            return None
        candidate = (self.static / path.lstrip("/")).resolve()
        if candidate.is_dir():
            candidate = candidate / "index.html"
        if not candidate.is_file() or self.static not in candidate.parents:
            return None
        return candidate


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802 — BaseHTTPRequestHandler's spelling
        server: CatalogServer = self.server  # type: ignore[assignment]
        path = urlsplit(self.path).path
        body, kind, name = server.objects.get(path), "object", path
        if body is None:
            static = server.static_file(path)
            # The served file names the type: a request for the root is an index.html, and
            # answering it as an octet-stream makes the browser download the app.
            if static:
                body, kind, name = static.read_bytes(), "static", static.name
        status = 200 if body is not None else 404
        server.record(path, status, kind if body is not None else "missing")
        self.send_response(status)
        self.send_header("Content-Type", content_type(name) if body is not None else "text/plain")
        self.end_headers()
        self.wfile.write(body if body is not None else b"not found")

    def log_message(self, *_args: object) -> None:
        pass


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", choices=sorted(CATALOGS), default="web-assemble")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--static", type=Path, help="A built site to serve on the same origin.")
    parser.add_argument("--log", type=Path, help="JSONL request log; its first line names what is served.")
    args = parser.parse_args()
    if args.static and not (args.static / "index.html").is_file():
        raise SystemExit(
            f"{args.static}/index.html is missing. Build it first:\n"
            "  cd builder/app && npm run build:web"
        )
    server = CatalogServer(CATALOGS[args.catalog](), args.port, args.static, args.log)
    print(f"{server.origin} serving {len(server.objects)} pinned objects", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
