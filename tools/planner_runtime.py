"""Planner release identity and file verification."""

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
from urllib.request import Request, urlopen

# Optional data layers. Each is one archive `maps/NAME.pmtiles`, baked by `tools/planner_NAME.py` when
# the region recipe has the field NAME. The value is a metadata key that every complete archive has.
DATA_LAYERS = {"snow": "seasons", "climate": "years", "sun": "sun_format"}


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def open_url(url, timeout=60):
    request = Request(url) if isinstance(url, str) else url
    request.add_header("User-Agent", "OpenBikeComputer/1.0")
    return urlopen(request, timeout=timeout)


def storage_files(document):
    """Map the published object names to their stored size and digest."""
    result = {}
    for name, item in document["files"].items():
        path = PurePosixPath(name)
        if path.is_absolute() or ".." in path.parts or path.as_posix() != name or "\\" in name:
            raise ValueError("Invalid release file")
        stored = item["transport"] if document.get("grid") else item
        if document.get("grid") and not re.fullmatch(r"[a-f0-9]{64}", stored.get("sha256", "")):
            raise ValueError("Invalid release checksum")
        key = "objects/" + stored["sha256"] if document.get("grid") else name
        if key in result and result[key] != stored:
            raise ValueError("Conflicting release object")
        result[key] = stored
    return result


def public_metadata(document):
    """Small immutable pointers let tile requests avoid a world-sized catalogue."""
    if not document.get("grid"):
        return {}
    result = {"public/grid.json": encoded({"format": 2, "map_zoom": document["grid"]["map_zoom"]})}
    for name, item in document["files"].items():
        if name.startswith(("maps/tiles/", "maps/assets/", "routes/tiles/")) or name in {"maps/basemap.json", "maps/places.json", "maps/overlays.json", "maps/terrain.json", "device/catalog.json",
                                                                         *(f"maps/{layer}.json" for layer in DATA_LAYERS)}:
            result["public/" + name + ".json"] = encoded({**item["transport"], "decoded_bytes": item["bytes"]})
    return result


def release(data, include_sources=True):
    path = data / "release.json"
    document = json.loads(path.read_bytes())
    identity = digest(path)
    if document["format"] != 1 or not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", document["region"]):
        raise ValueError("Unsupported planner release")
    files = {**storage_files(document), **(document.get("source_files", {}) if include_sources else {})}
    for name, item in files.items():
        file = data / name
        if not file.resolve().is_relative_to(data.resolve()) or not re.fullmatch(r"[a-f0-9]{64}", item["sha256"]):
            raise ValueError("Invalid release file")
        if file.stat().st_size != item["bytes"] or digest(file) != item["sha256"]:
            raise ValueError(f"Release checksum mismatch: {name}")
    return identity, document
