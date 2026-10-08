"""Compose planner release and offline selection metadata from small grid indexes."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile

from . import planner_geo as geo, planner_grid as grid, planner_offline as offline, planner_runtime as runtime, step_request

MAP_KINDS = {"basemap", "places", "overlays", "terrain", *runtime.DATA_LAYERS}


def compose(indexes, options):
    required = {"basemap", "places", "overlays", "terrain", "routing", "pois", "addresses", "assets", "model", "fonts"}
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
    metadata, cells = {}, {}
    for component in ("pois", "addresses"):
        index = indexes[component]
        item = index["metadata"]
        if (item["schema"] != 5 or not item["time_zone"] or item["bounds"] != options["bounds"]
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
    routing_cells = {cell["id"]: cell for cell in routing["cells"]}
    if not set(routing_cells) <= set(cells):
        raise ValueError("Routing cell coverage differs")
    for name, descriptor in routing_cells.items():
        if descriptor["bounds"] != cells[name]["bounds"]:
            raise ValueError("Routing and search cells differ")
        cells[name]["routing"] = descriptor
    map_blocks = []
    for kind in MAP_KINDS & indexes.keys():
        index = indexes[kind]
        info = index["metadata"]
        if kind != "terrain" and info["bounds"] != options["bounds"]:
            raise ValueError("Map coverage differs")
        if kind in runtime.DATA_LAYERS and runtime.DATA_LAYERS[kind] not in info:
            raise ValueError("Incomplete optional map metadata")
        if kind == "places" and info["osm_sha256"] != osm:
            raise ValueError("Places use another OSM snapshot")
        if kind == "overlays" and info["routing_package"] != files["routing/blocks.json"]["sha256"]:
            raise ValueError("Overlay tiles use another routing package")
        if kind in {"basemap", "places", "overlays", "terrain"}:
            for name in index["files"]:
                if name.endswith(".pmtiles"):
                    tile = list(map(int, Path(name).stem.split("-")))
                    map_blocks.append({"kind": kind, "tile": tile, "bounds": geo.tile_bounds(*tile), "files": [name]})
    terrain = indexes["terrain"]
    identities = json.loads(terrain["metadata"]["source_sha256"])
    if not set(data["source_sha256"][1:]) <= set(identities):
        raise ValueError("Routing and maps use different terrain sources")
    if "sun" in indexes and indexes["sun"]["metadata"].get("terrain_grid_sha256") != hashlib.sha256(runtime.encoded(terrain)).hexdigest():
        raise ValueError("Sun uses another terrain grid")
    shared = {name: name for name in indexes["assets"]["files"]}
    shared.update(indexes["fonts"]["aliases"])
    for name in {*(name for cell in cells.values() for name in cell["files"]), *shared.values()}:
        if name not in files:
            raise ValueError(f"Missing grid file {name}")
    release = {"format": 1, **options, "osm_sha256": osm,
        "routing_package": files["routing/blocks.json"]["sha256"], "profiles": sorted(data["metrics"]),
        "terrain_bounds": terrain["metadata"]["bounds"], "terrain_attribution": terrain["metadata"]["attribution"]}
    catalog = {"format": 3, "release": release, "routing_source": graph["source"],
        "map_blocks": sorted(map_blocks, key=lambda block: (block["kind"], block["tile"])),
        "cells": list(cells.values()), "shared": shared, "files": dict(files),
        "zoom": grid.ZOOM, "map_zoom": 11}
    search = {"format": 3, "metadata": metadata, "cells": [
        {**cell, "files": [name.removeprefix("search/") for name in cell["files"] if name.startswith("search/")]}
        for cell in cells.values()]}
    for cell in search["cells"]:
        cell.pop("routing", None)
    return files, release, catalog, search


def step(request):
    indexes = {}
    for selected in request["layers"].values():
        path, = selected.values()
        index = json.loads(Path(path).read_bytes())
        if index["kind"] in indexes:
            raise ValueError("Duplicate grid index")
        indexes[index["kind"]] = index
    files, release, catalog, search = compose(indexes, request["options"])
    output = Path(request["output"])
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        for name, document in [(f"search/{release['region']}.grid.json", search), ("offline/catalog.json", catalog)]:
            if name == "offline/catalog.json":
                catalog["files"] = dict(files)
            path = work / Path(name).name
            path.write_bytes(runtime.encoded(document))
            files[name] = offline.pack_file(path, output / "objects")
    document = {**release, "grid": {"format": 2, "zoom": grid.ZOOM, "map_zoom": 11}, "files": files}
    (output / "release.json").write_bytes(runtime.encoded(document))
    for name, data in runtime.public_metadata(document).items():
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    (output / "index.json").write_bytes(runtime.encoded({"format": 1, "kind": "index", "files": files}))
    step_request.metrics(request, {"files": len(files)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
