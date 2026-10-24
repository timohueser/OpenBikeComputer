#!/usr/bin/env python3
"""The `obc data` steps `device/reference/<leaf>` and `planner/reference`: the tiles of a version
of `terrain` that reach a map leaf or the planner terrain, as one reference archive.

    uv run --locked --offline --group terrain-reference python host/obc-dem/reference/merge.py --step

The request reads some tiles of a version of `terrain` and the `index.json` of their pool. The
version holds the national models merged already, so the step cuts its tiles and its index to the
option `tiles`. With the option `pool: "mean"`, it reads the mean tiles under `mean/`. The layer is
the archive below `reference/`.
"""

import json
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[2]))

from ingest.archive import MEAN  # noqa: E402
from ingest.lattice import tile_path  # noqa: E402
from tools import step_request  # noqa: E402


def merge(request) -> dict:
    options = request["options"]
    root = Path(request["output"]) / "reference"
    root.mkdir()
    prefix = f"{MEAN}/" if options.get("pool") == MEAN else ""
    files = step_request.files(request, options["source"])
    index = json.loads(files[f"{prefix}index.json"].read_text(encoding="utf-8"))
    keep = sorted(tile for tile in set(options["tiles"]) if tile in index["tiles"])
    for tile in keep:
        path = tile_path(root, *(int(part) for part in tile.split("/")))
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(files[f"{prefix}16/{tile}.tif"], path)
    contributors = {tile: index["contributors"][tile] for tile in keep}
    used = sorted({key for held in contributors.values() for key in held})
    cut = {**index, "sources": {key: index["sources"][key] for key in used},
           "tiles": {tile: index["tiles"][tile] for tile in keep}, "contributors": contributors,
           "sha256": {tile: index["sha256"][tile] for tile in keep}}
    (root / "index.json").write_text(json.dumps(cut, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
                                     encoding="utf-8")
    return {"tiles": len(keep), "sources": used}


if __name__ == "__main__":
    if sys.argv[1:] != ["--step"]:
        sys.exit(__doc__)
    request = step_request.read()
    step_request.metrics(request, merge(request))
