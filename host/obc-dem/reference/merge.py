#!/usr/bin/env python3
"""The `obc data` step `maps/reference/<leaf>`: the national terrain models that reach one map
leaf, merged into one reference archive.

    uv run --locked --offline --group terrain-reference python host/obc-dem/reference/merge.py --step

The request reads, per model, the pooled tiles that `ingest.py fetch` wrote for the archive
tiles of the leaf. The options name each model with its version and credit, and the archive
tiles that the terrain cells of the leaf read. The models go into an empty archive best first,
by `PRIORITY`: the archive that `ingest.py ingest` of each model in that order writes, cut to
those tiles. The layer is the archive below `reference/`.
"""

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[2]))

from ingest.archive import ingest_rasters, priority_rank, rebuild_index  # noqa: E402
from ingest.sources import SOURCES  # noqa: E402
from ingest.sources.base import READABLE  # noqa: E402
from tools import step_request  # noqa: E402


def key(source: str) -> str:
    """The ingest key of a source of data/sources.toml: `dtm-ch` is `ch`."""

    return source.removeprefix("dtm-")


def merge(request) -> dict:
    options = request["options"]
    output = Path(request["output"])
    root = output / "reference"
    root.mkdir()
    views = output.with_name("view")
    views.mkdir()
    keep = set(options["tiles"])
    for model in sorted(options["models"], key=lambda model: priority_rank(key(model["source"]))):
        row = SOURCES[key(model["source"])]
        view = step_request.view(step_request.files(request, model["source"]), views / model["source"])
        # A `.none` file is a tile where the model has no height: it is no raster.
        rasters = sorted(path for path in view.rglob("*") if path.suffix.lower() in READABLE)
        # The day of the data is the version of its fetch, never the day of the build.
        fetched = model["version"]
        ingest_rasters(rasters, row, root, fetched, row.fill(model["credit"], fetched), keep)
    index = rebuild_index(root)
    return {"tiles": len(index["tiles"]), "sources": sorted(index["sources"])}


if __name__ == "__main__":
    if sys.argv[1:] != ["--step"]:
        sys.exit(__doc__)
    request = step_request.read()
    step_request.metrics(request, merge(request))
