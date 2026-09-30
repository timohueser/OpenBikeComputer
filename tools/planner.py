#!/usr/bin/env python3
"""Prepare, publish, deploy, or preview regional planner data."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen

try:
    from . import planner_maps as maps
except ImportError:
    import planner_maps as maps

ROOT = maps.ROOT
SEARCH = ROOT / "apps/planner-search"
REGION = "baden-wuerttemberg"
CACHE = Path.home() / ".cache/obcm"


def run(*command, **kwargs):
    return maps.run(*command, cwd=ROOT, **kwargs)


def latest_basemap():
    result = run("curl", "-fsSL", "https://build-metadata.protomaps.dev/builds.json", capture_output=True, text=True)
    keys = [b["key"] for b in json.loads(result.stdout)
            if b["version"].startswith("4.") and re.fullmatch(r"\d{8}\.pmtiles", b["key"])]
    if not keys:
        raise ValueError("No compatible Protomaps build. Pass setup --basemap URL.")
    return "https://build.protomaps.com/" + max(keys)


def setup(args):
    for executable in ["node", "npm", "uv", "gh", "cargo", "curl", args.pmtiles]:
        if not shutil.which(executable):
            raise ValueError(f"Install {executable}, then repeat obc planner setup.")
    run("cargo", "build", "--release", "-p", "route-build", "-p", "route-server",
        "-p", "obc-dem", "--features", "route-build/obc-terrain")
    run(sys.executable, SEARCH / "setup.py", "--build-data", "--region", REGION,
        "--data-dir", args.data_dir / "search")
    if not maps.DATA.exists():
        maps.prepare(argparse.Namespace(pmtiles=args.pmtiles, basemap=args.basemap or latest_basemap(),
                     terrain="https://download.mapterhorn.com/planet.pmtiles", bbox=maps.bounds(maps.BW_BOUNDS)))
    route = args.data_dir / "routing"
    if not route.exists():
        source = args.osm or CACHE / "geofabrik/europe_germany_baden-wuerttemberg-latest.osm.pbf"
        if not source.is_file():
            if args.osm:
                raise ValueError(f"OSM input does not exist: {source}")
            source.parent.mkdir(parents=True, exist_ok=True)
            partial = source.with_suffix(".download")
            run("curl", "--fail", "--location", "--retry", "3",
                "--output", partial, "https://download.geofabrik.de/europe/germany/baden-wuerttemberg-latest.osm.pbf")
            partial.rename(source)
        run(ROOT / "target/release/obc-dem", "fetch", "--bbox", "47.5,7.45,49.85,10.5", "--out", args.dem_dir)
        with tempfile.TemporaryDirectory(prefix=".routing-", dir=args.data_dir) as stage:
            output = Path(stage) / "routing"
            command = [ROOT / "target/release/route-build", source, "--output", output,
                       "--region", REGION, "--country", "DE", "--bounds", maps.BW_BOUNDS,
                       "--profiles", "all", "--dem", args.dem_dir]
            if args.reference:
                command += ["--reference", args.reference]
            print("Building BW routing with local elevation data. This can take a long time.", flush=True)
            run(*command)
            output.rename(route)
    run(ROOT / "target/release/route-server", route, "--build-overlays")
    verify(args, full=True)
    print("Setup complete. Run: obc planner", flush=True)


def verify(args, full=False):
    manifest = maps.check_bundle(full)
    if manifest["bounds"] != maps.bounds(maps.BW_BOUNDS):
        raise ValueError("The map bundle must cover Baden-Württemberg.")
    route = args.data_dir / "routing"
    routing = json.loads((route / "manifest.json").read_text())
    if routing["region"] != REGION or routing["bounds"] != maps.bounds(maps.BW_BOUNDS):
        raise ValueError("The route package must cover Baden-Württemberg. Repeat setup with a fresh data directory.")
    if not (route / "overlays.sqlite").is_file():
        raise ValueError("Missing overlay index. Run obc planner setup.")
    for name in ["touring", "road", "gravel", "mtb", "hiking"]:
        if name not in routing["metrics"]:
            raise ValueError(f"Route package lacks {name}.")
    search = args.data_dir / "search"
    with sqlite3.connect(f'{(search / (REGION + ".sqlite")).as_uri()}?mode=ro', uri=True) as db:
        if db.execute("SELECT value FROM metadata WHERE key='schema'").fetchone() != ('1',):
            raise ValueError("Incomplete search package. Repeat setup.")
        if full and db.execute('PRAGMA quick_check').fetchone() != ('ok',):
            raise ValueError("Search database failed verification.")
    for path in [search / "model" / name for name in
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
        "OBC_SEARCH_REGIONS": REGION,
        "VITE_PLANNER_SEARCH_REGIONS": REGION,
        "VITE_PLANNER_DATA_URL": "",
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
                if status["parser"]["ready"] and [r["id"] for r in status["regions"]] == [REGION]:
                    print(f"Ready: http://127.0.0.1:{args.port}/planner.html (Baden-Württemberg, local)", flush=True)
                    return
            except (OSError, ValueError, KeyError):
                pass
            time.sleep(0.25)
        raise RuntimeError("Planner services did not become ready. Check the service output above.")

    maps.supervise(commands, env, ready)


def main():
    if len(sys.argv) > 1 and sys.argv[1] in {"prepare", "publish", "deploy", "rollback", "site-config"}:
        try: from .planner_release import main as release_main
        except ImportError: from planner_release import main as release_main
        release_main(sys.argv[1:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup", "serve", "verify"], nargs="?", default="serve")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get(
        "OBC_PLANNER_DATA", str(Path.home() / ".cache/obc/planner" / REGION)))
    parser.add_argument("--pmtiles", default=os.environ.get("PMTILES", "pmtiles"))
    parser.add_argument("--basemap", help="Override the available Protomaps v4 build for a new map bundle")
    parser.add_argument("--osm", type=Path, help="Use an existing BW OSM PBF for routing")
    parser.add_argument("--dem-dir", type=Path, default=CACHE / "dem")
    reference = os.environ.get("OBC_REFERENCE_ARCHIVE")
    if not reference and (Path.home() / "obc-reference/index.json").is_file():
        reference = str(Path.home() / "obc-reference")
    parser.add_argument("--reference", type=Path, default=reference)
    parser.add_argument("--port", type=int, default=4175)
    parser.add_argument("--tile-port", type=int, default=8789)
    parser.add_argument("--route-port", type=int, default=8787)
    parser.add_argument("--search-port", type=int, default=8786)
    args = parser.parse_args()
    args.data_dir = args.data_dir.expanduser().resolve()
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
