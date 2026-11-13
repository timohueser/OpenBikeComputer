"""Compose the planner release document and the search grid from the small grid indexes."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile

from . import planner_grid as grid, planner_offline as offline, planner_runtime as runtime, step_request

MAP_KINDS = {"basemap", "places", "overlays", "terrain", *runtime.DATA_LAYERS}
GRID = {"format": 3, "zoom": grid.ZOOM}


def compose(indexes, options):
    required = {"basemap", "places", "overlays", "terrain", "routing", "routing-cells", "pois", "addresses", "assets",
                "model", "fonts"}
    if not required <= indexes.keys():
        raise ValueError("Missing mandatory grid index")
    files = {}
    for index in indexes.values():
        if index["format"] != 1:
            raise ValueError("Unsupported grid index")
        for name, entry in index["files"].items():
            runtime.relative_path(name)
            for item in (entry, entry["transport"]):
                if (type(item["bytes"]) is not int or item["bytes"] < 0
                        or not re.fullmatch(r"[a-f0-9]{64}", item["sha256"])):
                    raise ValueError("Invalid grid file identity")
            if entry["transport"]["encoding"] not in {"identity", "gzip"}:
                raise ValueError("Unsupported grid file encoding")
            if name in files and files[name] != entry:
                raise ValueError("Conflicting grid files")
            files[name] = entry
    routing = indexes["routing"]
    graph = routing["graph"]
    data = graph["data"]
    if data["bounds"] != options["bounds"] or data["region"] != options["region"]:
        raise ValueError("Routing region or coverage differs")
    osm = data["source_sha256"][0]
    # The identity of the route package: specs/route-package.md.
    package = files["routing/directory.bin"]["sha256"]
    metadata, cells = {}, {}
    for component in ("pois", "addresses"):
        index = indexes[component]
        item = index["metadata"]
        if (item["schema"] != 6 or not item["time_zone"] or item["bounds"] != options["bounds"]
                or item["osm_sha256"] != osm or item["component"] != component):
            raise ValueError("Search component, source or coverage differs")
        if metadata:
            if item["schema"] != metadata["schema"] or item["time_zone"] != metadata["time_zone"]:
                raise ValueError("Search schema or time zone differs")
        else:
            metadata = {**item, "component": "all", "counts": {}}
        for kind, count in item["counts"].items():
            metadata["counts"][kind] = metadata["counts"].get(kind, 0) + count
        component_cells = {cell["id"]: cell for cell in index["cells"]}
        expected = dict(grid.cells(options["bounds"]))
        if set(component_cells) != set(expected):
            raise ValueError("Search cell coverage differs")
        for name, bounds in expected.items():
            cell = component_cells[name]
            if cell["bounds"] != bounds:
                raise ValueError("Search cell bounds differ")
            target = cells.setdefault(name, {"id": name, "bounds": bounds, "files": []})
            target["files"].extend(cell["files"])
    for cell in cells.values():
        cell["files"].append(f"routes/tiles/{cell['id']}.json")
    for kind in MAP_KINDS & indexes.keys():
        index = indexes[kind]
        info = index["metadata"]
        if kind != "terrain" and info["bounds"] != options["bounds"]:
            raise ValueError("Map coverage differs")
        if kind in runtime.DATA_LAYERS and runtime.DATA_LAYERS[kind] not in info:
            raise ValueError("Incomplete optional map metadata")
        if kind == "places" and info["osm_sha256"] != osm:
            raise ValueError("Places use another OSM snapshot")
        if kind == "overlays" and info["routing_package"] != package:
            raise ValueError("Overlay tiles use another routing package")
    terrain = indexes["terrain"]
    identities = json.loads(terrain["metadata"]["source_sha256"])
    if not set(data["source_sha256"][1:]) <= set(identities):
        raise ValueError("Routing and maps use different terrain sources")
    for name in (name for cell in cells.values() for name in cell["files"]):
        if name not in files:
            raise ValueError(f"Missing grid file {name}")
    release = {"format": 1, **options, "osm_sha256": osm,
        "routing_package": package, "profiles": sorted(data["metrics"]),
        "terrain_bounds": terrain["metadata"]["bounds"], "terrain_attribution": terrain["metadata"]["attribution"]}
    search = {"format": runtime.INDEX_FORMAT, "metadata": metadata, "cells": [
        {"id": cell["id"], "bounds": cell["bounds"],
         "files": [name.removeprefix("search/") for name in cell["files"] if name.startswith("search/")]}
        for cell in cells.values()]}
    return files, release, list(cells.values()), search


def described(data, index):
    """The entry of a step index in `indexes` of the release document."""
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "folders": sorted({name.split("/")[0] for name in index["files"]})}


def document(release, indexes, files):
    """The release document, format 2: the header, the step `indexes` and the `files` that no step index lists."""
    return {**release, "format": 2, "grid": GRID, "indexes": indexes, "files": files}


def read(layers):
    """The grid index of each step of `layers` by its kind, and its entry in `indexes` by its step."""
    indexes, named = {}, {}
    for name, selected in sorted(layers.items()):
        path, = selected.values()
        data = Path(path).read_bytes()
        index = json.loads(data)
        if index["kind"] in indexes:
            raise ValueError("Duplicate grid index")
        indexes[index["kind"]], named[name] = index, described(data, index)
    return indexes, named


def step(request):
    indexes, named = read(request["layers"])
    files, release, _, search = compose(indexes, request["options"])
    output = Path(request["output"])
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        path = Path(temporary) / "search.grid.json"
        path.write_bytes(runtime.encoded(search))
        own = {f"search/{release['region']}.grid.json": offline.pack_file(path, output / "objects")}
    (output / "release.json").write_bytes(runtime.encoded(document(release, named, own)))
    for name, data in runtime.public_metadata({**files, **own}).items():
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    step_request.metrics(request, {"files": len(files) + len(own), "indexes": len(named)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
