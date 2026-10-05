#!/usr/bin/env python3
"""Prepare, publish, deploy, or preview regional planner data."""

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
import time
from urllib.request import urlopen

if not __package__:  # `obc planner` runs this file as a script.
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools import data_registry, planner_bake, planner_cleanup, planner_deploy, planner_maps as maps, planner_prepare, r2
from tools import planner_release as releases
from tools.planner_runtime import DATA_LAYERS
from tools.planner_components import Cache

ROOT = maps.ROOT
SEARCH = ROOT / "apps/planner-search"
REGION = "baden-wuerttemberg-switzerland"
RECIPES = ROOT / "tools/planner-regions"
# The local preview commands. Each holds the lock of its data directory.
LOCAL = {"setup", "serve", "verify"}


def run(*command, **kwargs):
    return maps.run(*command, cwd=ROOT, **kwargs)


def setup(args):
    planner_bake.prepare(args)
    print(f"Setup complete. Run: obc planner serve --region {args.region}", flush=True)


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
                     maps.APP / "node_modules/vite/package.json", ROOT / "target/release/route-server"]:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"Missing {path}. Run obc planner setup.")
    if full:
        run(ROOT / "target/release/route-server", route, "--verify")
    return manifest


def verify_local(args):
    verify(args, full=True)
    print("Local planner data verified.")


def preview_config(args, manifest):
    """The planner config of the local services: the fields of a catalogue entry, with root-relative
    paths that the planner resolves against its page, so localhost and 127.0.0.1 both stay on one origin."""
    files = f"/@fs{args.data_dir}/maps"
    return {"name": args.name, "region": args.region, "bounds": manifest["bounds"],
            "basemap": f"pmtiles://{files}/basemap.pmtiles", "places": files + "/places.pmtiles",
            "overlays": files + "/overlays.pmtiles", "terrain": "/tiles/terrain/{z}/{x}/{y}.webp",
            "attribution": data_registry.credits(*releases.MAP_SOURCES), "terrain_attribution": manifest["terrain_attribution"],
            # Without its archive, the planner offers no such data layer.
            "layers": {layer: f"{files}/{layer}.pmtiles" for layer in DATA_LAYERS if f"{layer}.pmtiles" in manifest["files"]},
            "glyphs": files + "/assets/fonts/{fontstack}/{range}.pbf", "sprites": files + "/assets/sprites/v4",
            "routing": "/routing", "search": "/api/planner-search",
            # The routing step bakes the route catalog of the region, which the planner reads as one file.
            "routes": f"/@fs{args.data_dir}/routing/route-catalog.json"}


def serve(args):
    manifest = verify(args)
    ports = [args.port, args.tile_port, args.route_port, args.search_port]
    if len(set(ports)) != len(ports):
        raise ValueError("Each planner service needs a different port.")
    for port in ports:
        maps.check_port(port)
    tiles, routing = f"http://127.0.0.1:{args.tile_port}", f"http://127.0.0.1:{args.route_port}"
    env = {
        **os.environ,
        "VITE_PLANNER_CONFIG": json.dumps(preview_config(args, manifest)),
        # Vite serves the map files and the route catalog, and proxies terrain and routing.
        "OBC_PLANNER_MAPS_DIR": str(args.data_dir / "maps"),
        "OBC_PLANNER_ROUTES_FILE": str(args.data_dir / "routing/route-catalog.json"),
        "OBC_PLANNER_TILES_URL": tiles,
        "OBC_PLANNER_ROUTING_URL": routing,
        "ROUTE_LISTEN": f"127.0.0.1:{args.route_port}",
        "OBC_SEARCH_PORT": str(args.search_port),
        "OBC_SEARCH_DATA": str(args.data_dir / "search"),
        "OBC_SEARCH_PYTHON": str(SEARCH / ".venv/bin/python"),
        "OBC_SEARCH_REGIONS": args.region,
    }
    commands = [([str(ROOT / "target/release/route-server"), str(args.data_dir / "routing")], ROOT),
                (["node", "server.mjs"], SEARCH),
                ([args.pmtiles, "serve", str(args.data_dir / "maps"), "--interface=127.0.0.1",
                  f"--port={args.tile_port}", f"--public-url={tiles}"], ROOT),
                (["npm", "run", "dev", "--", "--mode", "web", "--host", "127.0.0.1", "--port", str(args.port), "--strictPort"], maps.APP)]

    def ready(children):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and all(p.poll() is None for p in children):
            try:
                with urlopen(f"http://127.0.0.1:{args.port}/api/planner-search/status", timeout=1) as response:
                    status = json.load(response)
                with urlopen(routing + "/health", timeout=1):
                    pass
                if status["parser"]["ready"] and [r["id"] for r in status["regions"]] == [args.region]:
                    print(f"Ready: http://127.0.0.1:{args.port}/planner.html ({args.region}, local)", flush=True)
                    return
            except (OSError, ValueError, KeyError):
                pass
            time.sleep(0.25)
        raise RuntimeError("Planner services did not become ready. Check the service output above.")

    maps.supervise(commands, env, ready)


def inventory(args):
    print(json.dumps(list(Cache(args.source_cache).inventory()), indent=2))


def grid(args):
    if not args.input_release: raise ValueError("Provide --input-release for grid publication")
    # The grid producers need the map packages of their own environment.
    try:
        run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
            "python", "-m", "tools.planner_blocks", args.input_release, args.data_dir, "--source-cache", args.source_cache)
    except subprocess.CalledProcessError as error:
        raise ValueError(f"grid step failed with exit status {error.returncode}") from None


COMMANDS = {"serve": serve, "setup": setup, "verify": verify_local, "prepare": planner_bake.prepare,
            "plan": planner_bake.prepare, "inventory": inventory, "grid": grid, "publish": releases.publish,
            "deploy": planner_deploy.deploy, "finalize": planner_cleanup.finalize}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=COMMANDS, nargs="?", default="serve")
    parser.add_argument("--region", default=REGION, choices=sorted(p.stem for p in RECIPES.glob("*.json")))
    parser.add_argument("--recipe", type=Path, help="Default: the recipe of --region")
    parser.add_argument("--data-dir", type=Path, help="Default: OBC_PLANNER_DATA/REGION")
    parser.add_argument("--port", type=int, default=4175)
    parser.add_argument("--tile-port", type=int, default=8789)
    parser.add_argument("--route-port", type=int, default=8787)
    parser.add_argument("--search-port", type=int, default=8786)
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
    parser.add_argument("--public-url", default="https://maps.openbikecomputer.com")
    parser.add_argument("--tiles-url", default="https://tiles.openbikecomputer.com")
    parser.add_argument("--api-url", default="https://releases.openbikecomputer.com")
    parser.add_argument("--site-origin", default="https://openbikecomputer.com")
    parser.add_argument("--host", default=os.environ.get("OBC_PLANNER_HOST"), help="VPS for deploy and finalize: USER@HOST")
    parser.add_argument("--apply", action="store_true", help="Upload, install, or remove what the command previews")
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
        args.bounds, args.name = recipe["bounds"], recipe["name"]
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
            parser.exit(130, "Planner command interrupted. Check the active catalogue before retrying.\n")
    except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error, subprocess.CalledProcessError, r2.Refuse) as error:
        hint = "\nUse obc planner setup to prepare local data and dependencies." if args.command in LOCAL else ""
        parser.exit(1, f"planner: {error}{hint}\n")


if __name__ == "__main__":
    main()
