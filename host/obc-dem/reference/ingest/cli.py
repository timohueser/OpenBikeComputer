"""The two steps of `obc data source prepare terrain`: one block of 4 x 4 archive tiles, and the index.

The archive is one format: pooled bare-earth height on the OBCT lattice at 2^6 µdeg, in tiles of
2^16 µdeg: the maximum in `int16` metres, and the mean in `int32` decimetres. A prepared version
packs them into one GeoTIFF per 2^18 µdeg block and pool.
Every country branch is an adapter in `ingest/sources/`. The baker knows the tiles only, and
`README.md` beside this file holds the contract both sides read.

A source adapter has one job: give the shared tail rasters that cover the box, in any CRS
and any dtype. The tail pools them onto the lattice by the contract's rule — a lattice
pixel keeps the maximum of the source pixels whose centres lie in it — so a rock tower one
source pixel wide survives the 7 m step.
"""

import argparse
import fcntl
import json
import os
import shutil
import signal
import sys
import threading
import time
from pathlib import Path

import numpy as np

from .archive import (MEAN, coarse, local_rasters, padded, pixel_digest, pool_tile, priority_rank, samples,
                      write_raster)
from .lattice import (BLOCK_PX, BLOCK_TILES, MEAN_NODATA, NODATA, TILE_PX, Refuse, block_path, block_window,
                      check_world, covering_window, in_block, tile_bounds, tile_window)
from .sources import SOURCES, ManualSource


def registered(key: str):
    if key not in SOURCES:
        raise Refuse(f"unknown source `{key}`; this tool implements {', '.join(sorted(SOURCES))}")
    return SOURCES[key]


def command_fetch(args) -> int:
    """One block of a version of `terrain`, for `obc data source prepare`: each tile `<ti>-<tj>=<keys>`
    of `--block`, its models (keys joined by commas) pooled onto the lattice and folded worst first,
    so that the best model with a height holds each pixel. It writes `18/<bi>/<bj>.tif`, the maximum,
    and `mean/18/<bi>/<bj>.tif`, the mean; then `<bi>-<bj>.tiles`, the sources of each tile with a
    height, best first, and its bias samples (`samples`); then `<bi>-<bj>.mean.sha256` and
    `<bi>-<bj>.sha256`, the digests of their pixels. Where no model has a height in the block it
    writes only the empty `<bi>-<bj>.none`.

    The raw rasters of a tile go to `--work` and go when the tile is pooled; `--work` is removed at
    the end whatever happens, so disk holds only the rasters of the tiles that run and the store
    never sees them. `--shared` is what the blocks of one prepare share: the summit check of each
    model, and a file that a whole region is published in.
    """

    def stop(*_):
        # `uv run` forwards the SIGTERM that its process group also gets: a second one must not
        # cut short the `finally` that removes the raw rasters.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        sys.exit(143)

    signal.signal(signal.SIGTERM, stop)
    stop_with_parent()
    bi, bj = parse_tile(args.block, "--block")
    work, out = Path(args.work), Path(args.out)
    shared = Path(args.shared) if args.shared else work / "shared"
    tiles = {}
    for text in args.tiles:
        tile, _, keys = text.partition("=")
        ti, tj = parse_tile(tile, "a tile")
        if (ti // BLOCK_TILES, tj // BLOCK_TILES) != (bi, bj) or not keys:
            raise Refuse(f"{text} is not <ti>-<tj>=<keys> of a tile of block {args.block}")
        check_world(tile_bounds(ti, tj), text)
        tiles[ti, tj] = keys.split(",")
    blocks = {MEAN: np.full((BLOCK_PX, BLOCK_PX), MEAN_NODATA, "int32"),
              "": np.full((BLOCK_PX, BLOCK_PX), NODATA, "int16")}
    made = {}
    try:
        for (ti, tj), keys in sorted(tiles.items()):
            into = in_block(ti, tj)
            source = np.full((TILE_PX, TILE_PX), -1, "int8")
            ranked = sorted(set(keys), key=priority_rank)
            for rank in reversed(range(len(ranked))):
                pooled = pool_model(registered(ranked[rank]), ti, tj, work / f"{ti:04}-{tj:04}" / ranked[rank], shared)
                if pooled is None:
                    continue
                for root, tile, nodata in (("", pooled[0], NODATA), (MEAN, pooled[1], MEAN_NODATA)):
                    held = tile != nodata
                    blocks[root][into][held] = tile[held]
                    if root == "":
                        source[held] = rank
            if (source >= 0).any():
                made[f"{ti:04}/{tj:04}"] = {"sources": [key for k, key in enumerate(ranked) if (source == k).any()],
                                            "samples": samples(blocks[MEAN][into])}
        out.mkdir(parents=True, exist_ok=True)
        name = f"{bi:04}-{bj:04}"
        if not made:
            (out / f"{name}.none").touch()
            return 0
        (out / f"{name}.tiles").write_text(json.dumps(made, sort_keys=True), encoding="utf-8")
        for root, mark in ((MEAN, "mean.sha256"), ("", "sha256")):
            written, path = block_path(work / "pooled" / root, bi, bj), block_path(out / root, bi, bj)
            write_raster(written, block_window(bi, bj), blocks[root])
            path.parent.mkdir(parents=True, exist_ok=True)
            os.replace(written, path)
            # Last, and the maximum last of all: a digest names a whole block.
            (out / f"{name}.{mark}").write_text(pixel_digest(blocks[root]), encoding="utf-8")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return 0


def pool_model(source, ti: int, tj: int, work: Path, shared: Path):
    """The maximum and the mean of `source` in tile `ti`, `tj`, or `None` where it has no height."""

    source.shared = shared
    check_summit(source, shared)
    box = tile_bounds(ti, tj)
    if coarse(source, ti, tj):
        box = padded(box, source)
    try:
        return pool_tile(rasters(source, box, work), source, ti, tj)
    finally:
        shutil.rmtree(work, ignore_errors=True)


# How far the highest pooled height near a row's summit may stand from the surveyed one. The geoid
# stands 20 m to 60 m above the ellipsoid over Europe and its islands, so an ellipsoidal or wrongly
# referenced model misses by more; a 5 m model reads a sharp summit a few metres low.
SUMMIT_TOLERANCE_M = 10.0
# The half side of the box read around a summit, in degrees: about 1 km of latitude. Each summit
# of a row is the highest point within it, so a coordinate a few hundred metres off still finds it.
SUMMIT_BOX = 0.01


def check_summit(source, shared: Path) -> None:
    """Refuse a model whose heights miss its row's `summit`, before any of its heights are used.

    One prepare checks a model once: the tile that checks first writes `checks/<key>.ok` in
    `shared`, under a lock, and the others read it.
    """

    if source.summit is None:
        return
    checks = shared / "checks"
    checks.mkdir(parents=True, exist_ok=True)
    passed = checks / f"{source.key}.ok"
    with (checks / f"{source.key}.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if passed.exists():
            return
        name, lon, lat, metres = source.summit
        box = (lon - SUMMIT_BOX, lat - SUMMIT_BOX, lon + SUMMIT_BOX, lat + SUMMIT_BOX)
        near = covering_window(box)
        work = checks / source.key
        heights = []
        try:
            found = rasters(source, box, work)
            for ti in range(near.row0 // TILE_PX, (near.row0 + near.rows - 1) // TILE_PX + 1):
                for tj in range(near.col0 // TILE_PX, (near.col0 + near.cols - 1) // TILE_PX + 1):
                    pooled = pool_tile(found, source, ti, tj)
                    if pooled is None:
                        continue
                    tile, top = tile_window(ti, tj), (ti + 1) * TILE_PX
                    rows = slice(max(0, top - near.row0 - near.rows), max(0, top - near.row0))
                    cols = slice(max(0, near.col0 - tile.col0), max(0, near.col0 + near.cols - tile.col0))
                    part = pooled[0][rows, cols]
                    heights.extend(part[part != NODATA].tolist())
        finally:
            shutil.rmtree(work, ignore_errors=True)
        if not heights:
            raise Refuse(f"{source.key}: the model has no height within {SUMMIT_BOX}° of {name} ({lat}, "
                         f"{lon}); the row's summit or its extent is wrong")
        highest = max(heights)
        if abs(highest - metres) > SUMMIT_TOLERANCE_M:
            raise Refuse(f"{source.key}: {name} is {metres} m, and the highest height of the model within "
                         f"{SUMMIT_BOX}° of it is {highest} m; its heights are not the orthometric metres "
                         f"of `{source.vertical_datum}`")
        passed.touch()


def rasters(source, box, work: Path):
    """The rasters of `source` that cover `box`, fetched or read from the delivery."""

    if isinstance(source, ManualSource):
        # A delivery is like a credential: the environment names its directory and the datum
        # that the owner read in the order's metadata.
        prefix = f"OBC_REFERENCE_{source.key.upper().replace('-', '_')}"
        base = Path(os.environ.get(f"{prefix}_INPUT", "").strip())
        # `obc data` runs the fetch in the repository root, not where the variable was set.
        if base.name and not base.is_absolute():
            raise Refuse(f"{prefix}_INPUT is `{base}`: give the absolute path of the delivery")
        if not base.name or not base.is_dir():
            raise Refuse(f"{source.key} is ordered by hand: set {prefix}_INPUT to the directory of the delivery")
        # A portal that publishes orthometric and ellipsoidal products side by side cannot say which
        # an order held, and the two stand tens of metres apart, which is the size of a lift.
        if source.confirm_datum and os.environ.get(f"{prefix}_DATUM", "").strip() != source.confirm_datum:
            raise Refuse(f"{source.key} delivers more than one vertical datum: read the order's metadata and set "
                         f"{prefix}_DATUM={source.confirm_datum} when it says so; nothing here converts an "
                         "ellipsoidal height")
        return local_rasters(source, base, box, work)
    source.require_credential()
    # Outside its `extent` the product has no data, so only the overlap is downloaded.
    west, south, east, north = source.extent
    box = (max(box[0], west), max(box[1], south), min(box[2], east), min(box[3], north))
    return source.fetch(box, work) if box[0] < box[2] and box[1] < box[3] else []


def command_index_terrain(args) -> int:
    """`index.json` of a version of `terrain`: the index of the version before (`--base`), where the
    tiles of `--keys` (tile id to its sources, best first) replace the tiles of the blocks of
    `--blocks` (block ids), the blocks that this version made. The models of `--models` were
    fetched on `--fetched`. A model keeps the `glo30_bias_m` of the version before, else takes the
    one of `--biases` (model to metres)."""

    base = json.loads(Path(args.base).read_text(encoding="utf-8")) if args.base else {}
    keys = json.loads(Path(args.keys).read_text(encoding="utf-8"))
    remade = set(json.loads(Path(args.blocks).read_text(encoding="utf-8")))

    def block(tile):
        ti, tj = (int(part) for part in tile.split("/"))
        return f"{ti // BLOCK_TILES:04}/{tj // BLOCK_TILES:04}"

    contributors = {tile: held for tile, held in base.get("contributors", {}).items() if block(tile) not in remade}
    contributors.update(keys)
    sources = dict(base.get("sources", {}))
    for key in filter(None, args.models.split(",")):
        source = registered(key)
        bias = {"glo30_bias_m": sources[key]["glo30_bias_m"]} if "glo30_bias_m" in sources.get(key, {}) else {}
        sources[key] = {"product": source.product, "attribution": source.credit(args.fetched),
                        "licence": source.licence, "vertical_datum": source.vertical_datum, "fetched": args.fetched,
                        **bias}
    for key, metres in json.loads(Path(args.biases).read_text(encoding="utf-8")).items():
        sources[key] = {**sources[key], "glo30_bias_m": metres}
    used = {key for held in contributors.values() for key in held}
    index = {"block_log2": 18, "step_log2": 6, "tile_log2": 16, "contributors": contributors,
             "sources": {key: facts for key, facts in sources.items() if key in used}}
    Path(args.out).write_text(json.dumps(index, sort_keys=True, separators=(",", ":"), ensure_ascii=False),
                              encoding="utf-8")
    return 0


def stop_with_parent() -> None:
    """Stop this process when the process `OBC_PARENT_PID` names ends, so an orphan never removes
    the work directory of a later run."""

    parent = int(os.environ.get("OBC_PARENT_PID") or 0)

    def watch():
        while True:
            time.sleep(2)
            try:
                os.kill(parent, 0)
            except ProcessLookupError:
                os.kill(os.getpid(), signal.SIGTERM)
                return

    if parent:
        threading.Thread(target=watch, daemon=True).start()


def parse_tile(text: str, what: str) -> tuple[int, int]:
    i, _, j = text.partition("-")
    if not (len(i) == len(j) == 4 and i.isdigit() and j.isdigit()):
        raise Refuse(f"{what} {text} is <i>-<j>, two four-digit indices")
    return int(i), int(j)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    fetch = commands.add_parser("fetch", help="one block of a version of terrain into --out, for obc data")
    fetch.add_argument("tiles", nargs="+", help=f"<ti>-<tj>=<keys>: a tile and its source keys joined by commas, "
                                                f"of {', '.join(sorted(SOURCES))}")
    fetch.add_argument("--block", required=True, help="<bi>-<bj>, the block of 4 x 4 archive tiles")
    fetch.add_argument("--work", required=True, help="where the raw rasters go; removed at the end")
    fetch.add_argument("--out", required=True, help="where the pooled block goes")
    fetch.add_argument("--shared", help="what the blocks of one prepare share; default: inside --work")
    fetch.set_defaults(run=command_fetch)

    tiles = commands.add_parser("index-terrain", help="index.json of a version of terrain, for obc data")
    tiles.add_argument("--base", help="index.json of the version before")
    tiles.add_argument("--keys", required=True, help="a JSON object of tile id to its sources, best first")
    tiles.add_argument("--blocks", required=True, help="a JSON list of the ids of the blocks that the version made")
    tiles.add_argument("--models", required=True, help="the source keys that this version fetched")
    tiles.add_argument("--biases", required=True, help="a JSON object of source key to its GLO-30 bias in metres")
    tiles.add_argument("--fetched", required=True, help="the day of the data, YYYY-MM-DD")
    tiles.add_argument("--out", required=True, help="the index.json to write")
    tiles.set_defaults(run=command_index_terrain)

    args = parser.parse_args(argv)
    try:
        return args.run(args)
    except Refuse as refusal:
        print(f"ingest: {refusal}", file=sys.stderr)
        return 1

