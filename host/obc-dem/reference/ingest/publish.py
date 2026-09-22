"""The R2 seam: the archive's place in the maps bucket, and the plans `publish` and
`mirror` run.

The bucket itself is defined once for the repository, in `tools/r2.py`, which `obc r2 rm`
is the other user of. A delete tool that disagreed with this one about where the archive
sits would delete in the wrong place, so both read the same definition.

`run_rclone` is the one seam the tests replace, so the commands reach it through this module
and not through a name bound at import time.
"""

import sys
from pathlib import Path

from .lattice import Refuse

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "tools"))
try:
    import r2
finally:
    sys.path.pop(0)


ARCHIVE_PREFIX = r2.ARCHIVE_PREFIX
Remote = r2.Remote


def r2_remote() -> Remote:
    """The archive's place on R2, from the `OBC_R2_*` variables of `obc.local`."""

    try:
        bucket = r2.bucket_remote()
    except r2.Refuse as exc:
        raise Refuse(str(exc)) from exc
    return Remote(f"{bucket.path}/{r2.archive_prefix()}", bucket.env)


def run_rclone(argv: list[str], env: dict[str, str], capture: bool = False) -> str:
    """Spawn rclone with the remote in its environment. The one seam the tests replace."""

    try:
        return r2.run_rclone(argv, env, capture)
    except r2.Refuse as exc:
        raise Refuse(str(exc)) from exc


def publish_plan(root: Path, remote: Remote, staging: Path) -> list[list[str]]:
    """The four rclone calls of a publish, in order: upload, ask, fetch, publish.

    Every transfer is `copy`, so a publish only ever adds: an archive of one box cannot
    delete another region's tiles. The index is the only file a consumer reads before it
    knows what exists, so it goes last, and it goes up as the merge of what is already on
    R2 with this archive. The `lsf` call is how a first publish is told from a later one,
    so an empty bucket needs no failed download. `--checksum` makes a re-run idempotent: a
    tile whose bytes are already there is skipped whatever its timestamp says.
    """

    return [
        ["copy", str(root), remote.path, "--checksum", "--exclude", "/index.json"],
        ["lsf", remote.path, "--include", "index.json"],
        ["copy", remote.path, str(staging), "--include", "/index.json"],
        ["copy", str(staging / "index.json"), remote.path, "--checksum"],
    ]


def merge_index(published: dict | None, local: dict) -> dict:
    """The index to publish: what is on R2, plus this archive, this archive winning.

    A publish adds tiles, so the index it uploads must describe every tile on R2 and not
    only the box that was ingested last. Where both hold a tile, this archive wins: its
    bytes are the ones the first call just uploaded.
    """

    if not published:
        return local
    if (published.get("schema"), published.get("step_log2"), published.get("tile_log2")) != (1, 6, 16):
        raise Refuse("the index on R2 is not this contract, so publish refuses to merge with it")
    tiles = dict(sorted({**published.get("tiles", {}), **local.get("tiles", {})}.items()))
    digests = dict(sorted({**published.get("sha256", {}), **local.get("sha256", {})}.items()))
    credits = dict(sorted({**published.get("contributors", {}), **local.get("contributors", {})}.items()))
    sources = {**published.get("sources", {}), **local.get("sources", {})}
    # Every source that holds a pixel anywhere, not only the best one per tile: a secondary
    # source's attribution has to survive the next publish of its neighbour.
    held = {key for keys in credits.values() for key in keys} | set(tiles.values())
    return {
        **local,
        "sources": {key: sources[key] for key in sorted(sources) if key in held},
        "tiles": tiles,
        "contributors": credits,
        "sha256": digests,
    }


def mirror_plan(root: Path, remote: Remote, listing: Path) -> list[str]:
    return ["copy", remote.path, str(root), "--files-from", str(listing), "--checksum"]
