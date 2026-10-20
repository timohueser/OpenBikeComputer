#!/usr/bin/env python3
"""The `obc data` steps `device/reference/<leaf>` and `planner/reference`: the national terrain
models that reach a map leaf or the planner terrain, merged into one reference archive.

    uv run --locked --offline --group terrain-reference python host/obc-dem/reference/merge.py --step

The request reads, per model, some tiles of a version of `terrain-reference/<key>`. The options
name each model with the day of its version and its credit, and the archive tiles. The models go
into an empty archive best first, by `PRIORITY`. For the maximum, that is the archive that
`ingest.py ingest` of each model in that order writes, cut to those tiles. With the option
`pool: "mean"`, the request reads the mean tiles, and a pixel keeps the best model that has it.
The layer is the archive below `reference/`.
"""

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[2]))

import numpy as np  # noqa: E402
import rasterio  # noqa: E402

from ingest.archive import (MEAN, ingest_rasters, priority_rank, rebuild_index, write_manifest,  # noqa: E402
                            write_tile)
from ingest.lattice import MEAN_NODATA, tile_path  # noqa: E402
from ingest.sources import SOURCES  # noqa: E402
from ingest.sources.base import READABLE  # noqa: E402
from tools import step_request  # noqa: E402


def key(source: str) -> str:
    """The ingest key of a source of data/sources.toml: `terrain-reference/ch` is `ch`."""

    return source.removeprefix("terrain-reference/")


def merge(request) -> dict:
    options = request["options"]
    output = Path(request["output"])
    root = output / "reference"
    root.mkdir()
    views = output.with_name("view")
    views.mkdir()
    keep = set(options["tiles"])
    mean = options.get("pool") == MEAN
    for model in sorted(options["models"], key=lambda model: priority_rank(key(model["source"]))):
        row = SOURCES[key(model["source"])]
        view = step_request.view(step_request.files(request, model["source"]), views / key(model["source"]))
        # The day of the data is the version of its fetch, never the day of the build.
        fetched = model["version"]
        credit = row.fill(model["credit"], fetched)
        if mean:
            fill_gaps(view / MEAN, row, root, fetched, credit, keep)
        else:
            rasters = sorted(path for path in view.rglob("*") if path.suffix.lower() in READABLE)
            ingest_rasters(rasters, row, root, fetched, credit, keep)
    index = rebuild_index(root, options.get("pool", "max"))
    return {"tiles": len(index["tiles"]), "sources": sorted(index["sources"])}


def fill_gaps(view: Path, row, root: Path, fetched: str, credit: str, keep: set[str]) -> None:
    """The mean tiles of one model in `view` into the gaps of the archive at `root`, and its
    manifest: the tiles where one of its pixels stays."""

    tiles = []
    for tile in sorted(keep):
        ti, tj = (int(part) for part in tile.split("/"))
        if not tile_path(view, ti, tj).is_file():
            continue
        with rasterio.open(tile_path(view, ti, tj)) as src:
            pixels = src.read(1)
        if tile_path(root, ti, tj).is_file():
            with rasterio.open(tile_path(root, ti, tj)) as src:
                held = src.read(1)
            pixels = np.where(held == MEAN_NODATA, pixels, held)
            if (pixels == held).all():
                continue
        write_tile(tile_path(root, ti, tj), ti, tj, pixels)
        tiles.append(tile)
    write_manifest(root, {"key": row.key, "country": row.country, "product": row.product,
                          "resolution_m": row.resolution_m, "licence": row.licence, "attribution": credit,
                          "vertical_datum": row.vertical_datum, "fetched": fetched, "tiles": tiles})


if __name__ == "__main__":
    if sys.argv[1:] != ["--step"]:
        sys.exit(__doc__)
    request = step_request.read()
    step_request.metrics(request, merge(request))
