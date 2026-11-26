#!/usr/bin/env python3
"""A digest-pinned catalog fixture on loopback, for the browser journeys of the web app.

`web_assemble()` is the journey's catalog, where every artifact is real, so a region download
from it assembles the checked-in cells into the checked-in map.

Every `bytes`, `sha256` and `url` is computed from the bytes this module serves, never copied from
a document.

Run it as a server for a browser test; it prints its origin first:

    python3 tools/fixture_catalog.py --catalog web-assemble
"""

from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
ASSEMBLE_FIXTURE = ROOT / "builder/wasm/tests/fixture"

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
    document carries for it (`obcc-catalog.md` §9)."""

    digest = hashlib.sha256(body).hexdigest()
    stem, _, extension = path.rpartition(".")
    url = f"{stem}.{digest}.{extension}"
    objects[url] = body
    return {"url": url, "bytes": len(body), "sha256": digest}


def _document(objects: dict[str, bytes], path: str, document: object) -> dict:
    return pin(objects, path, json.dumps(document).encode())


# The catalog of the browser journeys.
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

# Where this catalog is published on the server, and what the browser suite sets
# `VITE_CATALOG_URL` to.
PREFIX = "/catalog"
REGION_ID = "bridge-fixture"
REGION_NAME = "Bridge Fixture"
DATASET_ID = "fixture-raster"
DATASET_VERSION = "1"
SOURCES = [{"extract_id": "fixture", "snapshot": "2026-07-19"}]


def _cell_path(band: str, cell_id: str, extension: str) -> str:
    _log2, i, j = cell_id.split("/")
    return f"{PREFIX}/cells/{band}/{i}/{j}.{extension}"


def web_assemble() -> dict[str, bytes]:
    """The bridge fixture published as a catalog: schema, skin, cells, terrain, one region."""

    sidecar = json.loads((ASSEMBLE_FIXTURE / "cells.json").read_text())
    skin_doc = json.loads((ASSEMBLE_FIXTURE / "skin.json").read_text())
    dusk_doc = json.loads((ASSEMBLE_FIXTURE / "dark-skin.json").read_text())
    terrain_doc = json.loads((ASSEMBLE_FIXTURE / "terrain.json").read_text())
    objects: dict[str, bytes] = {}

    styles = sorted(skin_doc["styles"], key=lambda style: style["id"])

    # The cut's own schema, verbatim, plus the three things a catalog root states that a local
    # sidecar does not: an identity, the canonical style-id assignment, and the routing profiles.
    schema = dict(sidecar["schema"])
    schema.update(
        id="bridge-fixture",
        name="Bridge Fixture",
        description="The obc-builder-bridge bridge fixture's cut, published as a catalog schema.",
        styles=[{"id": s["id"], "feature_type": FEATURE_TYPES[s["id"]]} for s in styles],
    )
    schema["routing"] = {**schema.get("routing", {}), "profiles": []}
    schema_body = json.dumps(schema).encode()
    objects[f"{PREFIX}/schema.json"] = schema_body
    schema["sha256"] = hashlib.sha256(schema_body).hexdigest()

    def hosted_style(feature_type: str, style: dict) -> dict:
        def color(value: int | str) -> int:
            return int(value, 0) if isinstance(value, str) else value

        return {
            "feature_type": feature_type,
            "color": color(style["color"]), "weight": style["weight"], "z_index": style["z_index"],
            "priority": style.get("priority", 1),
            "line_style": style.get("line_style", "solid"),
            "fixed_width": style.get("fixed_width", False),
            "terrain_layer": style.get("terrain_layer", False),
            "color2": color(style["color2"]) if style.get("color2") is not None else None,
        }

    light_skin = {
        "id": "default",
        "name": skin_doc["name"],
        "description": "The cut's own styling, which is what the expected map was stamped with.",
        "version": 1,
        "marker_color": skin_doc["marker_color"],
        "styles": [hosted_style(FEATURE_TYPES[s["id"]], s) for s in styles],
    }
    dark_skin = {
        "id": dusk_doc["id"], "name": dusk_doc["name"],
        "description": "The authored dark styling carried by the checked bridge fixture.",
        "version": 1, "marker_color": dusk_doc["marker_color"],
        "styles": [
            hosted_style(FEATURE_TYPES[s["id"]], s)
            for s in sorted(dusk_doc["styles"], key=lambda style: style["id"])
        ],
    }

    # One cell index per band, each cell pinned by the bytes this server hands over.
    by_band: dict[str, list[dict]] = {band["id"]: [] for band in schema["bands"]}
    for cell in sidecar["cells"]:
        body = (ASSEMBLE_FIXTURE / cell["path"]).read_bytes()
        entry = {"id": cell["id"], "sources": SOURCES, "partial": cell["partial"]}
        entry.update(pin(objects, _cell_path(cell["band"], cell["id"], "obcm"), body))
        by_band[cell["band"]].append(entry)

    cell_index = []
    for band in sorted(schema["bands"], key=lambda b: -b["cell_log2"]):
        cells = by_band[band["id"]]
        document = {
            "schema_version": 3, "schema_sha256": schema["sha256"], "band": band["id"],
            "cells": cells, "known_empty": [],
        }
        ref = {"band": band["id"], "cell_log2": band["cell_log2"],
               "cell_count": len(cells), "known_empty_count": 0}
        ref.update(_document(objects, f"{PREFIX}/cells/{band['id']}/index.json", document))
        cell_index.append(ref)

    terrain_cells = []
    for cell in terrain_doc["cells"]:
        body = (ASSEMBLE_FIXTURE / cell["path"]).read_bytes()
        entry = {"id": cell["id"]}
        entry.update(pin(objects, _cell_path("terrain", cell["id"], "obcd"), body))
        terrain_cells.append(entry)
    terrain_index = {
        "schema_version": 3,
        "dataset_id": DATASET_ID, "dataset_version": DATASET_VERSION,
        "posting_log2": terrain_doc["posting_log2"], "cell_log2": terrain_doc["cell_log2"],
        "cells": terrain_cells, "known_empty": [],
    }
    terrain = {
        "dataset_id": DATASET_ID, "dataset_version": DATASET_VERSION,
        "posting_log2": terrain_doc["posting_log2"], "cell_log2": terrain_doc["cell_log2"],
        "attribution": "Synthetic raster cut by builder/wasm/examples/fixture.rs.",
        "cell_index": {"cell_count": len(terrain_cells), "known_empty_count": 0,
                       **_document(objects, f"{PREFIX}/cells/terrain/index.json", terrain_index)},
    }

    region_cells = {
        "schema_version": 3, "schema_sha256": schema["sha256"], "region_id": REGION_ID,
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
        "source": {
            "dataset_id": "fixture",
            "attribution": "Synthetic geometry cut by builder/wasm/examples/fixture.rs.",
            "license": "CC0-1.0",
            "license_url": "https://creativecommons.org/publicdomain/zero/1.0/",
        },
        "schema": schema,
        "skins": [light_skin, dark_skin],
        "regions": [region],
        "cell_index": cell_index,
        "terrain": terrain,
    }
    objects[f"{PREFIX}/catalog.json"] = json.dumps(catalog).encode()
    return objects


CATALOGS = {"web-assemble": web_assemble}




class CatalogServer(ThreadingHTTPServer):
    """Serves a pinned object set on loopback."""

    daemon_threads = True

    def __init__(self, objects: dict[str, bytes], port: int = 0) -> None:
        super().__init__(("127.0.0.1", port), _Handler)
        self.objects = objects

    @property
    def origin(self) -> str:
        return f"http://127.0.0.1:{self.server_port}"


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802 — BaseHTTPRequestHandler's spelling
        server: CatalogServer = self.server  # type: ignore[assignment]
        path = urlsplit(self.path).path
        body = server.objects.get(path)
        self.send_response(200 if body is not None else 404)
        self.send_header("Content-Type", content_type(path) if body is not None else "text/plain")
        self.end_headers()
        self.wfile.write(body if body is not None else b"not found")

    def log_message(self, *_args: object) -> None:
        pass


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", choices=sorted(CATALOGS), default="web-assemble")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    server = CatalogServer(CATALOGS[args.catalog](), args.port)
    print(f"{server.origin} serving {len(server.objects)} pinned objects", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
