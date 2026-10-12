#!/usr/bin/env python3
"""Prepare and verify regional planner data."""

import argparse
from contextlib import closing
import fcntl
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys

if not __package__:  # `obc planner` runs this file as a script.
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools import planner_bake, planner_maps as maps, planner_prepare
from tools import planner_release as releases
from tools.planner_components import Cache

ROOT = maps.ROOT
SEARCH = ROOT / "planner/search"
REGION = "baden-wuerttemberg-switzerland"
RECIPES = ROOT / "tools/planner-regions"
# The local data commands. Each holds the lock of its data directory.
LOCAL = {"setup", "verify"}


def run(*command, **kwargs):
    return maps.run(*command, cwd=ROOT, **kwargs)


def setup(args):
    planner_bake.prepare(args)
    print("Setup complete.", flush=True)


def current_overlays(data):
    """Whether the overlay tiles come from the overlay index of this routing package."""
    tiles, index = data / "maps/overlays.pmtiles", data / "routing/overlays.sqlite"
    if not tiles.is_file() or not index.is_file(): return False
    with closing(sqlite3.connect(f"{index.as_uri()}?mode=ro", uri=True)) as db:
        package = db.execute("SELECT package FROM metadata").fetchone()[0]
    return releases.archive_metadata(tiles).get("routing_package") == package


def verify(args, full=False):
    """The map manifest of the data directory, once its data can serve the region."""
    manifest = maps.check_bundle(args.data_dir / "maps", full)
    if manifest["bounds"] != args.bounds:
        raise ValueError(f"The map bundle must cover {args.region}.")
    route = args.data_dir / "routing"
    routing = json.loads((route / "manifest.json").read_text())
    if routing["region"] != args.region or routing["bounds"] != args.bounds:
        raise ValueError(f"The route package must cover {args.region}. Repeat setup with a fresh data directory.")
    if not current_overlays(args.data_dir):
        raise ValueError("Missing or stale overlay index or tiles. Run obc planner setup.")
    for name in ["touring", "road", "gravel", "mtb", "hiking"]:
        if name not in routing["metrics"]:
            raise ValueError(f"Route package lacks {name}.")
    search = args.data_dir / "search"
    databases = [search / component / (args.region + ".sqlite") for component in ("pois", "addresses")]
    if not any(path.exists() for path in databases): databases = [search / (args.region + ".sqlite")]
    for database in databases: releases.search_metadata(database, full)
    for path in [route / "route-catalog.json"] + [search / "model" / name for name in
                 ["model.int8.onnx", "tokenizer.json", "tokenizer_config.json", "labels.json"]] + [
                     SEARCH / ".venv/bin/python", SEARCH / "node_modules/opening_hours/package.json",
                     maps.APP / "node_modules/vite/package.json", ROOT / "target/release/planner-service"]:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"Missing {path}. Run obc planner setup.")
    if full:
        run(ROOT / "target/release/planner-service", route, "--verify")
    return manifest


def verify_local(args):
    verify(args, full=True)
    print("Local planner data verified.")


def inventory(args):
    print(json.dumps(list(Cache(args.source_cache).inventory()), indent=2))


def grid(args):
    if not args.input_release: raise ValueError("Provide --input-release for grid publication")
    # The grid producers need the map packages of their own environment.
    try:
        run("uv", "run", "--locked", "--group", "planner-maps",
            "python", "-m", "tools.planner_blocks", args.input_release, args.data_dir, "--source-cache", args.source_cache)
    except subprocess.CalledProcessError as error:
        raise ValueError(f"grid step failed with exit status {error.returncode}") from None


COMMANDS = {"setup": setup, "verify": verify_local, "prepare": planner_bake.prepare,
            "plan": planner_bake.prepare, "inventory": inventory, "grid": grid}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=COMMANDS)
    parser.add_argument("--region", default=REGION, choices=sorted(p.stem for p in RECIPES.glob("*.json")))
    parser.add_argument("--recipe", type=Path, help="Default: the recipe of --region")
    parser.add_argument("--data-dir", type=Path, help="Default: OBC_PLANNER_DATA/REGION")
    parser.add_argument("--input-release", type=Path, help="Verified regional bake to update or to partition with grid")
    parser.add_argument("--source-cache", type=Path, default=Path.home() / ".cache/obc/planner/sources")
    parser.add_argument("--component", action="append", help="Update one component and its dependencies; repeat for multiple components")
    parser.add_argument("--dry-run", action="store_true", help="Print component identities and reuse reasons without downloading or building")
    parser.add_argument("--osm", type=Path)
    parser.add_argument("--inputs", type=Path, help="Verified source-builder output directory")
    parser.add_argument("--dem-dir", type=Path, default=Path.home() / ".cache/obcm/dem")
    reference = os.environ.get("OBC_REFERENCE_ARCHIVE")
    if not reference and (Path.home() / "obc-reference/index.json").is_file(): reference = str(Path.home() / "obc-reference")
    parser.add_argument("--reference", type=Path, default=reference)
    parser.add_argument("--pmtiles", default=os.environ.get("PMTILES", "pmtiles"))
    parser.add_argument("--device-catalog", default=os.environ.get("OBC_CATALOG_URL", "https://maps.openbikecomputer.com/cell-catalog/catalog.json"))
    args = parser.parse_args(argv)
    args.recipe = args.recipe or RECIPES / f"{args.region}.json"
    args.data_dir = args.data_dir or Path(os.environ.get("OBC_PLANNER_DATA", Path.home() / ".cache/obc/planner")) / args.region
    for name in ["data_dir", "input_release", "recipe", "source_cache", "osm", "inputs", "dem_dir", "reference"]:
        value = getattr(args, name)
        if value is not None: setattr(args, name, value.expanduser().resolve())
    args.dry_run = args.dry_run or args.command == "plan"

    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        if args.command not in LOCAL:
            COMMANDS[args.command](args)
            return
        routing = args.data_dir / "routing/manifest.json"
        if routing.exists() and json.loads(routing.read_text())["region"] != args.region:
            parser.error(f"{args.data_dir} holds another region. Choose a data directory for {args.region}.")
        recipe = planner_prepare.recipe(args.recipe)
        args.bounds = recipe["bounds"]
        # The lock sits beside the data directory: setup needs that directory fresh.
        args.data_dir.parent.mkdir(parents=True, exist_ok=True)
        with (args.data_dir.parent / f".{args.data_dir.name}.lock").open("w") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError("This data directory is already in use. Stop its planner before setup or another launch.")
            COMMANDS[args.command](args)
    except KeyboardInterrupt:
        if args.command not in LOCAL:
            parser.exit(130, "Planner command interrupted. Run it again: completed parts resume.\n")
    except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error, subprocess.CalledProcessError) as error:
        hint = "\nUse obc planner setup to prepare local data and dependencies." if args.command in LOCAL else ""
        parser.exit(1, f"planner: {error}{hint}\n")


if __name__ == "__main__":
    main()
