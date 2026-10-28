"""The two steps of `obc data source prepare terrain`: one archive tile, and the index.

The archive is one format: pooled bare-earth height on the OBCT lattice at 2^6 µdeg, one
GeoTIFF per 2^16 µdeg tile: the maximum in `int16` metres, and the mean in `int32` decimetres.
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
import rasterio

from .archive import (MEAN, coarse, fold_tiles, local_rasters, padded, pool_tile, priority_rank, samples,
                      tile_digest, write_tile)
from .lattice import (MEAN_NODATA, NODATA, TILE_PX, Refuse, check_world, covering_window, pixel_index, tile_bounds,
                      tile_path, tile_window, udeg_floor)
from .sources import SOURCES, ManualSource


def registered(key: str):
    if key not in SOURCES:
        raise Refuse(f"unknown source `{key}`; this tool implements {', '.join(sorted(SOURCES))}")
    return SOURCES[key]


def command_fetch(args) -> int:
    """One archive tile of a version of `terrain`, for `obc data source prepare`: each model of
    `source` (keys joined by commas) pooled onto the lattice and folded into the tile of the
    version before (`--base`, an archive root, whose tile came from `--held`), best rule of
    `fold_tiles`. It writes `16/<ti>/<tj>.tif`, the maximum, and `mean/16/<ti>/<tj>.tif`, the mean;
    then `<ti>-<tj>.keys`, the sources of the tile best first, and `<ti>-<tj>.samples`, the bias
    samples of the mean (`samples`); then `<ti>-<tj>.mean.sha256` and
    `<ti>-<tj>.sha256`, the digests of their pixels. Where no model has a height in the tile it
    writes only the empty `<ti>-<tj>.none`, and the version keeps the tile before.

    The raw rasters go to `--work`, which is removed at the end whatever happens, so disk holds
    only the rasters of the tiles that run and the store never sees them. `--shared` is what the
    tiles of one prepare share: the summit check of each model, and a file that a whole region is
    published in.

    The models are folded best first by `PRIORITY`, whatever order they come in: a model fills only
    the gaps of the better ones, so a worse one folded first would keep pixels that a better one
    folded later cannot take back when a third, better than both, is already held.
    """

    def stop(*_):
        # `uv run` forwards the SIGTERM that its process group also gets: a second one must not
        # cut short the `finally` that removes the raw rasters.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        sys.exit(143)

    signal.signal(signal.SIGTERM, stop)
    stop_with_parent()
    ti, tj = parse_tile(args.tile)
    work, out = Path(args.work), Path(args.out)
    shared = Path(args.shared) if args.shared else work / "shared"
    check_world(tile_bounds(ti, tj), f"--tile {args.tile}")
    held = set(filter(None, (args.held or "").split(",")))
    folded = {}
    if args.base:
        for root in (MEAN, ""):
            path = tile_path(Path(args.base) / root, ti, tj)
            if path.is_file():
                with rasterio.open(path) as src:
                    folded[root] = src.read(1)
    try:
        fetched = False
        for key in sorted(args.source.split(","), key=priority_rank):
            pooled = pool_model(registered(key), ti, tj, work / key, shared)
            if pooled is None:
                continue
            fetched = True
            for root, tile, nodata in ((MEAN, pooled[1], MEAN_NODATA), ("", pooled[0], NODATA)):
                folded[root] = tile if root not in folded else fold_tiles(folded[root], tile, held, key, nodata)
            held.add(key)
        out.mkdir(parents=True, exist_ok=True)
        name = f"{ti:04}-{tj:04}"
        if not fetched:
            (out / f"{name}.none").touch()
            return 0
        (out / f"{name}.keys").write_text(",".join(sorted(held, key=priority_rank)), encoding="utf-8")
        (out / f"{name}.samples").write_text(samples(folded[MEAN]), encoding="utf-8")
        for root, mark in ((MEAN, "mean.sha256"), ("", "sha256")):
            written, path = tile_path(work / "pooled" / root, ti, tj), tile_path(out / root, ti, tj)
            write_tile(written, ti, tj, folded[root])
            path.parent.mkdir(parents=True, exist_ok=True)
            os.replace(written, path)
            # Last, and the maximum last of all: a digest names a whole tile.
            (out / f"{name}.{mark}").write_text(tile_digest(path), encoding="utf-8")
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
    return pool_tile(rasters(source, box, work), source, ti, tj)


# How far the highest pooled height near a row's summit may stand from the surveyed one. The geoid
# stands 20 m to 60 m above the ellipsoid over Europe and its islands, so an ellipsoidal or wrongly
# referenced model misses by more; a 5 m model reads a sharp summit a few metres low.
SUMMIT_TOLERANCE_M = 10.0
# The half side of the box read around a summit, in degrees: about 300 m.
SUMMIT_BOX = 0.003


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
        lon, lat, metres = source.summit
        ti, tj = (pixel_index(udeg_floor(value)) // TILE_PX for value in (lat, lon))
        west, south, east, north = tile_bounds(ti, tj)
        box = (max(west, lon - SUMMIT_BOX), max(south, lat - SUMMIT_BOX),
               min(east, lon + SUMMIT_BOX), min(north, lat + SUMMIT_BOX))
        work = checks / source.key
        try:
            pooled = pool_tile(rasters(source, box, work), source, ti, tj)
        finally:
            shutil.rmtree(work, ignore_errors=True)
        tile, near = tile_window(ti, tj), covering_window(box)
        top = tile.row0 + TILE_PX
        part = np.s_[top - near.row0 - near.rows:top - near.row0, near.col0 - tile.col0:near.col0 - tile.col0 + near.cols]
        held = pooled[0][part][pooled[0][part] != NODATA] if pooled is not None else np.array([])
        highest = int(held.max()) if held.size else None
        if highest is None or abs(highest - metres) > SUMMIT_TOLERANCE_M:
            raise Refuse(f"{source.key}: the summit at {lat}, {lon} is {metres} m, and the model reads "
                         f"{highest if highest is not None else 'no height'} there; its heights are not "
                         f"the orthometric metres of `{source.vertical_datum}`")
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
    """`index.json` of `--pool` in a version of `terrain`: the index of the version before
    (`--base`) with each tile of `--digests` (tile id to pixel digest) and `--keys` (tile id to its
    sources, best first). The models of `--models` were fetched on `--fetched`. A model keeps the
    `glo30_bias_m` of the version before, else takes the one of `--biases` (model to metres)."""

    base = json.loads(Path(args.base).read_text(encoding="utf-8")) if args.base else {}
    digests = json.loads(Path(args.digests).read_text(encoding="utf-8"))
    keys = json.loads(Path(args.keys).read_text(encoding="utf-8"))
    contributors = {**base.get("contributors", {}), **{tile: keys[tile] for tile in digests}}
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
    index = {"schema": 1, "step_log2": 6, "tile_log2": 16, "pool": args.pool,
             "sources": {key: facts for key, facts in sorted(sources.items()) if key in used},
             "tiles": {tile: held[0] for tile, held in sorted(contributors.items())},
             "contributors": dict(sorted(contributors.items())),
             "sha256": dict(sorted({**base.get("sha256", {}), **digests}.items()))}
    Path(args.out).write_text(json.dumps(index, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
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


def parse_tile(text: str) -> tuple[int, int]:
    ti, _, tj = text.partition("-")
    if not (len(ti) == len(tj) == 4 and ti.isdigit() and tj.isdigit()):
        raise Refuse(f"--tile {text} is <ti>-<tj>, two four-digit archive tile indices")
    return int(ti), int(tj)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    fetch = commands.add_parser("fetch", help="one archive tile of a version of terrain into --out, for obc data")
    fetch.add_argument("source", help=f"source keys joined by commas: {', '.join(sorted(SOURCES))}")
    fetch.add_argument("--tile", required=True, help="<ti>-<tj>, the archive tile")
    fetch.add_argument("--work", required=True, help="where the raw rasters go; removed at the end")
    fetch.add_argument("--out", required=True, help="where the pooled tile goes")
    fetch.add_argument("--base", help="the archive root of the version before, which holds the tile")
    fetch.add_argument("--held", help="the sources of the tile of --base, joined by commas")
    fetch.add_argument("--shared", help="what the tiles of one prepare share; default: inside --work")
    fetch.set_defaults(run=command_fetch)

    tiles = commands.add_parser("index-terrain", help="index.json of a version of terrain, for obc data")
    tiles.add_argument("--pool", choices=("max", MEAN), default="max", help="the pool of the tiles")
    tiles.add_argument("--base", help="index.json of the version before")
    tiles.add_argument("--digests", required=True, help="a JSON object of tile id to pixel digest")
    tiles.add_argument("--keys", required=True, help="a JSON object of tile id to its sources, best first")
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

