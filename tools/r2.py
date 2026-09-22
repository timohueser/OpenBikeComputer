#!/usr/bin/env python3
"""The R2 maps bucket: where it is, and the one guarded way to delete from it.

Everything the project publishes shares one bucket: the cell catalogue under
`OBC_R2_PREFIX`, the terrain reference archive beside it, and whatever else reached the
bucket by hand. The bucket is defined here, so `obc r2 rm` and the reference ingest tool
cannot disagree about where anything is.

`rm` replaces deleting in the Cloudflare console. The console shows a folder tree and
empties a folder on one click: it cannot tell a cell the live catalogue names from a stray
upload, and it asks nothing before it acts. Every guard below answers that.

    obc r2 rm cell-catalog/cells/fine/1204/1052.obcm
    obc r2 rm --prefix reference/v1/16/3410
    obc r2 rm <key> --apply --reason "bad ingest" --confirm "1 <key>"

A key is the object's full key inside the bucket, the way a listing prints it.
`run_rclone` is the one seam the tests replace, so every command reaches it through this
module and not through a name bound at import time.
"""

import argparse
import getpass
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


#: The terrain reference archive, under the catalogue prefix inside the bucket.
ARCHIVE_PREFIX = "reference/v1"

#: The bucket's removal history, at the bucket root so that no publish or purge sweeps it.
REMOVAL_LOG = "removed.jsonl"

#: The most objects one `rm` takes. The confirmation the owner types back guards a plan
#: only while a reader can still check it line by line, and both mistakes this tool exists
#: to stop — a mis-typed prefix and a folder emptied on one click — arrive as a long list.
#: A removal wider than this is a republish of the tree instead.
RM_CAP = 64

#: What every download reads before it knows what exists. Each needs `--i-mean-it`.
CATALOG_ROOT = ("catalog.json", "schema.json", "terrain.json", "LICENSE.txt")

#: The per-region cell indexes, by their folder inside the catalogue prefix.
CATALOG_INDEXES = "regions/"

#: What the owner has to run after a catalogue object goes, before a rider downloads again.
REPUBLISH = "obc bake publish --target r2"


class Refuse(Exception):
    """A condition the tool refuses to guess about, reported to the caller by name."""


@dataclass(frozen=True)
class Remote:
    """A place on R2, and the child environment that carries the credential to it."""

    path: str
    env: dict[str, str]


@dataclass(frozen=True)
class Target:
    """One object the plan names, as the live listing describes it."""

    key: str
    bytes: int
    modified: str


def need(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise Refuse(f"{name} is not set; source tools/obc.local, which holds the R2 credential")
    return value


def bucket_remote() -> Remote:
    """The bucket root, from the `OBC_R2_*` variables of `tools/obc.local`.

    The secret reaches rclone through the child's environment only: it is never an
    argument, because argv is readable by every process on the box.
    """

    endpoint = os.environ.get("OBC_R2_ENDPOINT") or f"https://{need('OBC_R2_ACCOUNT_ID')}.r2.cloudflarestorage.com"
    env = {
        "RCLONE_CONFIG_OBCR2_TYPE": "s3",
        "RCLONE_CONFIG_OBCR2_PROVIDER": "Cloudflare",
        "RCLONE_CONFIG_OBCR2_REGION": "auto",
        "RCLONE_CONFIG_OBCR2_ENDPOINT": endpoint,
        "RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID": need("OBC_R2_ACCESS_KEY_ID"),
        "RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY": need("OBC_R2_SECRET_ACCESS_KEY"),
        "RCLONE_CONFIG_OBCR2_NO_CHECK_BUCKET": "true",
    }
    return Remote(f"OBCR2:{need('OBC_R2_BUCKET')}", env)


def catalog_prefix() -> str:
    """The cell catalogue's key prefix inside the bucket, or `''` when none is configured."""

    return os.environ.get("OBC_R2_PREFIX", "").strip("/")


def archive_prefix() -> str:
    """The reference archive's key prefix inside the bucket."""

    return "/".join(part for part in (catalog_prefix(), ARCHIVE_PREFIX) if part)


def catalog_key(name: str = "catalog.json") -> str:
    """One of the catalogue's own documents, by its key inside the bucket."""

    prefix = catalog_prefix()
    return f"{prefix}/{name}" if prefix else name


def run_rclone(argv: list[str], env: dict[str, str], capture: bool = False) -> str:
    """Spawn rclone with the remote in its environment. The one seam the tests replace."""

    try:
        done = subprocess.run(["rclone", *argv], env={**os.environ, **env}, check=True,
                              capture_output=capture, text=capture)
        return done.stdout if capture else ""
    except FileNotFoundError as exc:
        raise Refuse("rclone is not on PATH — this tool reaches R2 through it "
                     "(https://rclone.org/install/)") from exc
    except subprocess.CalledProcessError as exc:
        raise Refuse(f"rclone {argv[0]} failed with status {exc.returncode}") from exc


def fetch_optional(remote: Remote, key: str, into: Path) -> Path | None:
    """One named object out of the bucket, or None when the bucket does not hold it.

    The `lsf` asks the object's own folder and not the bucket, so nothing here ever walks
    a prefix with millions of tiles under it just to find out whether one file exists.
    """

    folder, _, name = key.rpartition("/")
    place = f"{remote.path}/{folder}" if folder else remote.path
    if not run_rclone(["lsf", place, "--include", f"/{name}"], remote.env, capture=True).strip():
        return None
    run_rclone(["copyto", f"{remote.path}/{key}", str(into)], remote.env)
    return into


def listed_under(remote: Remote, prefix: str) -> list[str]:
    """Every object key under one prefix, from the live listing."""

    rows = run_rclone(["lsjson", f"{remote.path}/{prefix}", "--recursive", "--files-only"],
                      remote.env, capture=True)
    return sorted(f"{prefix}/{row['Path']}" for row in json.loads(rows or "[]"))


def live_facts(remote: Remote, staging: Path, keys: list[str]) -> dict[str, Target]:
    """Size and last-modified for exactly these keys. A key absent from R2 is absent here."""

    listing = staging / "wanted.txt"
    listing.write_text("".join(f"{key}\n" for key in keys), encoding="utf-8")
    rows = run_rclone(["lsjson", remote.path, "--recursive", "--files-only",
                       "--files-from", str(listing)], remote.env, capture=True)
    return {row["Path"]: Target(row["Path"], row.get("Size", 0), row.get("ModTime", ""))
            for row in json.loads(rows or "[]")}


def checked_prefix(text: str) -> str:
    """A `--prefix` that names a folder inside the bucket, or a refusal."""

    prefix = text.strip("/")
    if not prefix or any(part in ("", ".", "..") for part in prefix.split("/")):
        raise Refuse(f"--prefix {text!r} does not name a folder inside the bucket; "
                     "the bucket root is never a target")
    return prefix


def strings(document) -> set[str]:
    """Every string anywhere in a JSON document.

    The catalogue names its objects in several shapes and the shapes move with the schema.
    What does not move is that a published key appears verbatim, so the test is membership
    in every string the document holds. It protects more than it must, which is the safe
    direction for a delete.
    """

    found, stack = set(), [document]
    while stack:
        item = stack.pop()
        if isinstance(item, str):
            found.add(item)
        elif isinstance(item, dict):
            stack.extend(item.values())
        elif isinstance(item, list):
            stack.extend(item)
    return found


def inside_catalog(key: str) -> str | None:
    """The key as the catalogue names it, or None when the key is not catalogue content."""

    prefix = catalog_prefix()
    if key.startswith(f"{archive_prefix()}/"):
        return None
    if not prefix:
        return key
    return key[len(prefix) + 1:] if key.startswith(f"{prefix}/") else None


def protection(key: str, named: set[str]) -> str | None:
    """Why this key needs `--i-mean-it`, or None if nothing downstream depends on it."""

    if key == f"{archive_prefix()}/index.json":
        return "the reference archive index; a tile it does not name is not in the archive"
    inside = inside_catalog(key)
    if inside is None:
        return None
    if inside in CATALOG_ROOT or inside.startswith(CATALOG_INDEXES):
        return "the catalogue root; every download reads it before it knows what exists"
    if inside in named:
        return f"the live catalogue names it; downloads of it fail until `{REPUBLISH}`"
    return None


def reference_tile(key: str) -> str | None:
    """The `ti/tj` of a reference archive tile, or None for anything else."""

    head = f"{archive_prefix()}/16/"
    if not (key.startswith(head) and key.endswith(".tif")):
        return None
    ti, _, tj = key[len(head):-len(".tif")].partition("/")
    return f"{ti}/{tj}" if ti and tj and "/" not in tj else None


def prune_index(index: dict, tiles: list[str]) -> dict:
    """The reference index with `tiles` unnamed, and a source left holding nothing dropped."""

    named = set(tiles)
    kept = {tile: key for tile, key in index.get("tiles", {}).items() if tile not in named}
    credits = {tile: keys for tile, keys in index.get("contributors", {}).items() if tile not in named}
    digests = {tile: value for tile, value in index.get("sha256", {}).items() if tile not in named}
    held = {key for keys in credits.values() for key in keys} | set(kept.values())
    return {
        **index,
        "sources": {key: facts for key, facts in index.get("sources", {}).items() if key in held},
        "tiles": kept,
        "contributors": credits,
        "sha256": digests,
    }


def confirmation(keys: list[str]) -> str:
    """What `--confirm` must repeat: the count and the first key of *this* plan.

    Both halves come from the resolved plan and never from the command line, so a
    confirmation carried over from an earlier run names a count or a key that no longer
    matches, and the removal refuses.
    """

    return f"{len(keys)} {keys[0]}"


def removal_lines(targets: list[Target], reason: str, who: str, when: str) -> str:
    """The log a removal appends: who took each object out, when, how big it was, and why."""

    return "".join(json.dumps({
        "removed": when, "by": who, "key": target.key, "bytes": target.bytes, "reason": reason,
    }, sort_keys=True) + "\n" for target in targets)


def resolve(remote: Remote, args) -> list[str]:
    """The keys a removal names. Exact keys, or one prefix listed; never both, never wider."""

    if bool(args.key) == bool(args.prefix):
        raise Refuse("name objects by key, or one folder with --prefix; "
                     "`rm` takes one form and widens neither")
    if args.prefix:
        return listed_under(remote, checked_prefix(args.prefix))
    keys = sorted({key.strip("/") for key in args.key})
    if not all(keys):
        raise Refuse("an empty key is the bucket root, which is never a target")
    return keys


def unname_tiles(remote: Remote, staging: Path, tiles: list[str]) -> None:
    """Rewrite the reference index without these tiles, and send it before the objects go.

    A crash after this leaves a tile no index names, which nothing asks for, instead of an
    index entry with no tile behind it, which every baker would fetch and fail on.
    """

    key = f"{archive_prefix()}/index.json"
    if fetch_optional(remote, key, staging / "index.json") is None:
        raise Refuse(f"{key} is not on R2, so the tiles cannot be unnamed before they go")
    path = staging / "index.json"
    index = json.loads(path.read_text(encoding="utf-8"))
    if (index.get("schema"), index.get("step_log2"), index.get("tile_log2")) != (1, 6, 16):
        raise Refuse("the reference index on R2 is not this contract, so `rm` refuses to rewrite it")
    path.write_text(json.dumps(prune_index(index, tiles), indent=2, sort_keys=True,
                               ensure_ascii=False) + "\n", encoding="utf-8")
    run_rclone(["copyto", str(path), f"{remote.path}/{key}"], remote.env)


def append_log(remote: Remote, staging: Path, targets: list[Target], reason: str) -> None:
    """Append this removal to the bucket's history, before the objects themselves go."""

    log = staging / REMOVAL_LOG
    fetch_optional(remote, REMOVAL_LOG, log)
    when = datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    with log.open("a", encoding="utf-8") as handle:
        handle.write(removal_lines(targets, reason, getpass.getuser(), when))
    run_rclone(["copyto", str(log), f"{remote.path}/{REMOVAL_LOG}"], remote.env)


def command_rm(args) -> int:
    """Delete named objects from the bucket. It plans and stops unless `--apply` says so."""

    remote = bucket_remote()
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        keys = resolve(remote, args)
        if not keys:
            raise Refuse("that names no object in the bucket, so there is nothing to delete")
        if len(keys) > RM_CAP:
            raise Refuse(f"{len(keys)} objects is over the {RM_CAP} one `rm` takes; name a "
                         "narrower prefix, or publish the tree again instead of deleting")
        found = live_facts(remote, staging, keys)
        missing = [key for key in keys if key not in found]
        if missing:
            raise Refuse(f"the bucket does not hold {', '.join(missing[:5])}; nothing was "
                         "deleted, because the command line is not the one that was meant")
        targets = [found[key] for key in keys]
        tiles = sorted(tile for tile in (reference_tile(key) for key in keys) if tile)
        catalogued = [key for key in keys if inside_catalog(key) is not None]

        named = set()
        if catalogued and fetch_optional(remote, catalog_key(), staging / "catalog.json"):
            named = strings(json.loads((staging / "catalog.json").read_text(encoding="utf-8")))
        blocked = {key: why for key in keys if (why := protection(key, named))}

        print(f"{remote.path}: {len(keys)} object(s) to delete — key, bytes, modified")
        for target in targets:
            print(f"  {target.key}  {target.bytes}  {target.modified}")
        for key, why in blocked.items():
            mine = " (--i-mean-it)" if key in args.i_mean_it else ""
            print(f"  protected: {key} — {why}{mine}")
        if tiles:
            print(f"  {len(tiles)} reference tile(s): the archive index is rewritten first")
        if catalogued:
            print(f"  {len(catalogued)} catalogue object(s): republish after this with `{REPUBLISH}`")

        stale = [key for key in args.i_mean_it if key not in blocked]
        if stale:
            raise Refuse(f"--i-mean-it names {', '.join(stale)}, which this plan does not "
                         "protect; the confirmation belongs to another command line")
        unapproved = [key for key in blocked if key not in args.i_mean_it]
        if unapproved:
            raise Refuse("something downstream depends on "
                         f"{', '.join(unapproved)}; name each one in --i-mean-it to delete it")

        expected = confirmation(keys)
        if not args.apply:
            print(f'dry run — repeat with: --apply --reason "…" --confirm "{expected}"')
            return 0
        if not args.reason:
            raise Refuse("--apply needs --reason: the log is the only record of why an object went")
        if args.confirm != expected:
            raise Refuse(f'--confirm must repeat this plan\'s "{expected}"; it says '
                         f'"{args.confirm or ""}", so the plan is not the one that was reviewed')

        if tiles:
            unname_tiles(remote, staging, tiles)
        append_log(remote, staging, targets, args.reason)
        for key in keys:
            run_rclone(["deletefile", f"{remote.path}/{key}"], remote.env)

    print(f"deleted {len(keys)} object(s) from {remote.path}; {REMOVAL_LOG} holds the record")
    if catalogued:
        print(f"the live catalogue still names them — publish the tree now: {REPUBLISH}")
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="obc r2", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    remove = commands.add_parser(
        "rm", help="delete named objects from the R2 maps bucket",
        description="Delete named objects from the R2 maps bucket. It prints the plan and "
                    "stops; --apply does it and takes back the confirmation the plan "
                    f"printed. One call takes at most {RM_CAP} objects, because a typed "
                    "confirmation guards a plan only while a reader can still check it "
                    "line by line, and a wider removal is a republish of the tree.")
    remove.add_argument("key", nargs="*", help="an object key inside the bucket")
    remove.add_argument("--prefix", help="delete every object under this folder instead")
    remove.add_argument("--apply", action="store_true", help="delete; needs --reason and --confirm")
    remove.add_argument("--confirm", help="the string the dry run printed, in quotes")
    remove.add_argument("--reason", help="why these objects go; it is kept in the removal log")
    remove.add_argument("--i-mean-it", action="append", default=[], metavar="KEY",
                        help="delete this protected key as well; repeat it for each one")
    remove.set_defaults(run=command_rm)

    args = parser.parse_args(list(sys.argv[1:] if argv is None else argv))
    try:
        return args.run(args)
    except Refuse as refusal:
        print(f"obc r2: {refusal}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
