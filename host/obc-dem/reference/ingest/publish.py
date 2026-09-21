"""The R2 seam: the rclone remote, and the plans `publish` and `mirror` run.

`run_rclone` is the one seam the tests replace, so the commands reach it through this module
and not through a name bound at import time.
"""

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .lattice import Refuse


ARCHIVE_PREFIX = "reference/v1"

@dataclass(frozen=True)
class Remote:
    """The archive's place on R2, and the child environment that defines it."""

    path: str
    env: dict[str, str]


def r2_remote() -> Remote:
    """The `RCLONE_CONFIG_OBCR2_*` remote, from the `OBC_R2_*` variables of `obc.local`.

    The secret reaches rclone through the child's environment only: it is never an
    argument, because argv is readable by every process on the box.
    """

    def need(name: str) -> str:
        value = os.environ.get(name)
        if not value:
            raise Refuse(f"{name} is not set; source tools/obc.local, which holds the R2 credential")
        return value

    endpoint = os.environ.get("OBC_R2_ENDPOINT") or f"https://{need('OBC_R2_ACCOUNT_ID')}.r2.cloudflarestorage.com"
    bucket = need("OBC_R2_BUCKET")
    prefix = os.environ.get("OBC_R2_PREFIX", "").strip("/")
    env = {
        "RCLONE_CONFIG_OBCR2_TYPE": "s3",
        "RCLONE_CONFIG_OBCR2_PROVIDER": "Cloudflare",
        "RCLONE_CONFIG_OBCR2_REGION": "auto",
        "RCLONE_CONFIG_OBCR2_ENDPOINT": endpoint,
        "RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID": need("OBC_R2_ACCESS_KEY_ID"),
        "RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY": need("OBC_R2_SECRET_ACCESS_KEY"),
        "RCLONE_CONFIG_OBCR2_NO_CHECK_BUCKET": "true",
    }
    key_root = "/".join(part for part in (bucket, prefix, ARCHIVE_PREFIX) if part)
    return Remote(f"OBCR2:{key_root}", env)


def run_rclone(argv: list[str], env: dict[str, str], capture: bool = False) -> str:
    """Spawn rclone with the remote in its environment. The one seam the tests replace."""

    try:
        done = subprocess.run(["rclone", *argv], env={**os.environ, **env}, check=True,
                              capture_output=capture, text=capture)
        return done.stdout if capture else ""
    except FileNotFoundError as exc:
        raise Refuse("rclone is not on PATH — the publish and mirror steps need it "
                     "(https://rclone.org/install/)") from exc
    except subprocess.CalledProcessError as exc:
        raise Refuse(f"rclone {argv[0]} failed with status {exc.returncode}") from exc


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
