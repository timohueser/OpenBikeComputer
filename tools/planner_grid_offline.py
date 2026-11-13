"""Compose the offline catalog and one file for each cell from the small grid indexes."""

import argparse
from pathlib import Path
import tempfile

from . import (planner_grid as grid, planner_grid_index as index, planner_grid_pack as pack,
               planner_map_archive as archive, planner_runtime as runtime, step_request)

# The kinds of map packs that an offline area takes. The terrain packs below `13/` are TrailView's only.
PACKS = ("basemap", "places", "overlays", "terrain")
# The files of the route package that a phone plans and installs the cells of an area with.
ROUTING = ("routing/package.json", "routing/directory.bin")


def compose(indexes, options):
    files, release, cells, _ = index.compose(indexes, options)
    shared = {name: name for name in indexes["assets"]["files"]}
    shared.update(indexes["fonts"]["aliases"])
    shared.update({name: name for name in ROUTING})
    if not set(shared.values()) <= files.keys():
        raise ValueError("Missing shared grid file")
    packs = {}
    for kind in PACKS:
        for name in indexes[kind]["files"]:
            if name.endswith(".pmtiles") and Path(name).parent.as_posix() == f"maps/tiles/{kind}":
                z, x, y = map(int, Path(name).stem.split("-"))
                packs.setdefault((kind, z), {})[x, y] = name
    catalog = {"format": runtime.INDEX_FORMAT, "zoom": grid.ZOOM, "release": release,
               "cells": sorted(cell["id"] for cell in cells),
               "shared": shared, "assets": {name: files[name] for name in sorted(set(shared.values()))}}
    documents = {cell["id"]: {
        "format": runtime.INDEX_FORMAT, "id": cell["id"], "bounds": cell["bounds"],
        "files": {name: files[name] for name in cell["files"]},
        "packs": {name: files[name] for name in selected(packs, cell["bounds"])},
    } for cell in cells}
    return catalog, documents


def selected(packs, box):
    """The names of the packs that `selected` of planner_map_archive takes for `box`. Each zoom of a
    kind visits the smaller of the tiles of its window and its packs, so a large grid stays fast."""
    for (kind, z), tiles in packs.items():
        window = archive.tile_window(box, z, int(kind == "terrain"))
        if (window[2] - window[0]) * (window[3] - window[1]) < len(tiles):
            yield from (tiles[tile] for tile in archive.window_tiles(z, window) if tile in tiles)
        else:
            yield from (name for (x, y), name in tiles.items() if archive.inside(x, y, z, window))


def step(request):
    indexes, _ = index.read(request["layers"])
    catalog, cells = compose(indexes, request["options"])
    with tempfile.TemporaryDirectory(dir=Path(request["output"]).parent) as temporary:
        work = Path(temporary)
        documents = {"offline/catalog.json": catalog, **{f"offline/cells/{name}.json": cell for name, cell in cells.items()}}
        for name, document in documents.items():
            path = work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(runtime.encoded(document))
        pack.pack_index(request, "offline", {name: work / name for name in documents})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
