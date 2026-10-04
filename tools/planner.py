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

try:
    from . import planner_maps as maps, planner_prepare, planner_release as releases
except ImportError:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from tools import planner_maps as maps, planner_prepare, planner_release as releases

ROOT = maps.ROOT
SEARCH = ROOT / "apps/planner-search"
REGION = "baden-wuerttemberg-switzerland"
RECIPES = ROOT / "tools/planner-regions"


def run(*command, **kwargs):
    return maps.run(*command, cwd=ROOT, **kwargs)


def setup(args):
    releases.main(["prepare", "--recipe", str(RECIPES / f"{args.region}.json"), "--data-dir", str(args.data_dir),
                   "--pmtiles", args.pmtiles])
    print(f"Setup complete. Run: obc planner serve --region {args.region}", flush=True)


def current_overlays(route):
    """Whether the overlay tiles come from the overlay index of this routing package."""
    tiles, index = maps.DATA / "overlays.pmtiles", route / "overlays.sqlite"
    if not tiles.is_file() or not index.is_file(): return False
    with closing(sqlite3.connect(f"{index.as_uri()}?mode=ro", uri=True)) as db:
        package = db.execute("SELECT package FROM metadata").fetchone()[0]
    return releases.archive_metadata(tiles).get("routing_package") == package


def verify(args, full=False):
    manifest = maps.check_bundle(full)
    if manifest["bounds"] != args.bounds:
        raise ValueError(f"The map bundle must cover {args.region}.")
    route = args.data_dir / "routing"
    routing = json.loads((route / "manifest.json").read_text())
    if routing["region"] != args.region or routing["bounds"] != args.bounds:
        raise ValueError(f"The route package must cover {args.region}. Repeat setup with a fresh data directory.")
    if not current_overlays(route):
        raise ValueError("Missing or stale overlay index or tiles. Run obc planner setup.")
    for name in ["touring", "road", "gravel", "mtb", "hiking"]:
        if name not in routing["metrics"]:
            raise ValueError(f"Route package lacks {name}.")
    search = args.data_dir / "search"
    releases.search_metadata(search / (args.region + ".sqlite"), full)
    for path in [route / "route-catalog.json"] + [search / "model" / name for name in
                 ["model.int8.onnx", "tokenizer.json", "tokenizer_config.json", "labels.json"]] + [
                     SEARCH / ".venv/bin/python", SEARCH / "node_modules/opening_hours/package.json",
                     maps.APP / "node_modules/vite/package.json", ROOT / "target/release/route-server"]:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"Missing {path}. Run obc planner setup.")
    if full:
        run(ROOT / "target/release/route-server", route, "--verify")


def serve(args):
    verify(args)
    ports = [args.port, args.tile_port, args.route_port, args.search_port]
    if len(set(ports)) != len(ports):
        raise ValueError("Each planner service needs a different port.")
    for port in ports:
        maps.check_port(port)
    args.routing = f"http://127.0.0.1:{args.route_port}"
    commands, env = maps.preview(args)
    env.update({
        "ROUTE_LISTEN": f"127.0.0.1:{args.route_port}",
        "OBC_SEARCH_PORT": str(args.search_port),
        "OBC_SEARCH_DATA": str(args.data_dir / "search"),
        "OBC_SEARCH_PYTHON": str(SEARCH / ".venv/bin/python"),
        "OBC_SEARCH_REGIONS": args.region,
        "VITE_PLANNER_SEARCH_REGIONS": args.region,
        "VITE_PLANNER_DATA_URL": "",
        "VITE_PLANNER_REGION_NAME": args.name,
        # The routing step bakes the route catalog of the region, which the planner reads as one file.
        "OBC_PLANNER_ROUTES_FILE": str(args.data_dir / "routing/route-catalog.json"),
        "VITE_PLANNER_ROUTES_URL": "/@fs" + str(args.data_dir / "routing/route-catalog.json"),
        "OBC_QUERY_ROUTER": args.routing,
    })
    commands = [([str(ROOT / "target/release/route-server"), str(args.data_dir / "routing")], ROOT),
                (["node", "server.mjs"], SEARCH)] + commands

    def ready(children):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and all(p.poll() is None for p in children):
            try:
                with urlopen(f"http://127.0.0.1:{args.port}/api/planner-search/status", timeout=1) as response:
                    status = json.load(response)
                with urlopen(args.routing + "/health", timeout=1):
                    pass
                if status["parser"]["ready"] and [r["id"] for r in status["regions"]] == [args.region]:
                    print(f"Ready: http://127.0.0.1:{args.port}/planner.html ({args.region}, local)", flush=True)
                    return
            except (OSError, ValueError, KeyError):
                pass
            time.sleep(0.25)
        raise RuntimeError("Planner services did not become ready. Check the service output above.")

    maps.supervise(commands, env, ready)


def main():
    if len(sys.argv) > 1 and sys.argv[1] in {"prepare", "plan", "inventory", "grid", "publish", "deploy", "rollback", "finalize", "site-config"}:
        try: from .planner_release import main as release_main
        except ImportError: from tools.planner_release import main as release_main
        release_main(sys.argv[1:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup", "serve", "verify"], nargs="?", default="serve")
    parser.add_argument("--region", default=REGION, choices=sorted(p.stem for p in RECIPES.glob("*.json")))
    parser.add_argument("--data-dir", type=Path, help="Default: OBC_PLANNER_DATA/REGION")
    parser.add_argument("--pmtiles")
    parser.add_argument("--port", type=int, default=4175)
    parser.add_argument("--tile-port", type=int, default=8789)
    parser.add_argument("--route-port", type=int, default=8787)
    parser.add_argument("--search-port", type=int, default=8786)
    args = parser.parse_args()
    args.pmtiles = args.pmtiles or os.environ.get("PMTILES", "pmtiles")
    base = Path(os.environ.get("OBC_PLANNER_DATA", Path.home() / ".cache/obc/planner"))
    args.data_dir = (args.data_dir or base / args.region).expanduser().resolve()
    routing = args.data_dir / "routing/manifest.json"
    if routing.exists() and json.loads(routing.read_text())["region"] != args.region:
        parser.error(f"{args.data_dir} holds another region. Choose a data directory for {args.region}.")
    recipe = planner_prepare.recipe(RECIPES / f"{args.region}.json")
    args.bounds, args.name = recipe["bounds"], recipe.get("name", "")
    maps.DATA = args.data_dir / "maps"
    args.data_dir.mkdir(parents=True, exist_ok=True)
    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        with (args.data_dir / ".lock").open("w") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError("This data directory is already in use. Stop its planner before setup or another launch.")
            if args.command == "setup":
                setup(args)
            elif args.command == "verify":
                verify(args, full=True)
                print("Local planner data verified.")
            else:
                serve(args)
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner: {error}\nUse obc planner setup to prepare local data and dependencies.\n")


if __name__ == "__main__":
    main()
