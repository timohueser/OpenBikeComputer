#!/usr/bin/env python3
"""The `obc data` steps `device/reference/<leaf>` and `planner/reference`: the tiles of a version
of `terrain` that reach a map leaf or the planner terrain, as one reference archive.

    uv run --locked --offline --group terrain-reference python host/obc-dem/reference/merge.py --step

The request reads the blocks of some tiles of a version of `terrain` and its `index.json`. The
version holds the national models merged already, so the step cuts each tile of the option
`tiles` that the index names out of its block, and cuts the index to them. With the option
`pool: "mean"`, it reads the mean blocks under `mean/`. The layer is the archive below
`reference/`, whose index names the best source and the pixel digest of each tile.
"""

import json
import sys
from pathlib import Path

import rasterio

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[2]))

from ingest.archive import MEAN, pixel_digest, write_tile  # noqa: E402
from ingest.lattice import BLOCK_TILES, block_path, in_block, tile_path  # noqa: E402
from tools import step_request  # noqa: E402


def merge(request) -> dict:
    options = request["options"]
    root = Path(request["output"]) / "reference"
    root.mkdir()
    pool = MEAN if options.get("pool") == MEAN else "max"
    prefix = f"{MEAN}/" if pool == MEAN else ""
    files = step_request.files(request, options["source"])
    index = json.loads(files["index.json"].read_text(encoding="utf-8"))
    keep = sorted(tile for tile in set(options["tiles"]) if tile in index["contributors"])
    digests, blocks = {}, {}
    for tile in keep:
        ti, tj = (int(part) for part in tile.split("/"))
        blocks.setdefault((ti // BLOCK_TILES, tj // BLOCK_TILES), []).append((tile, ti, tj))
    for (bi, bj), tiles in sorted(blocks.items()):
        name = block_path(Path(prefix), bi, bj).as_posix()
        with rasterio.open(files[name]) as src:
            pixels = src.read(1)
        for tile, ti, tj in tiles:
            values = pixels[in_block(ti, tj)]
            write_tile(tile_path(root, ti, tj), ti, tj, values)
            digests[tile] = pixel_digest(values)
    contributors = {tile: index["contributors"][tile] for tile in keep}
    used = sorted({key for held in contributors.values() for key in held})
    cut = {"schema": 1, "step_log2": 6, "tile_log2": 16, "pool": pool,
           "sources": {key: index["sources"][key] for key in used},
           "tiles": {tile: held[0] for tile, held in contributors.items()}, "contributors": contributors,
           "sha256": digests}
    (root / "index.json").write_text(json.dumps(cut, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
                                     encoding="utf-8")
    return {"tiles": len(keep), "sources": used}


if __name__ == "__main__":
    if sys.argv[1:] != ["--step"]:
        sys.exit(__doc__)
    request = step_request.read()
    step_request.metrics(request, merge(request))
