#!/usr/bin/env python3
"""Prepare and serve a local planner map with the PMTiles deployment layout."""

import argparse
import hashlib
import io
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from urllib.request import urlopen
import zipfile


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "builder/app"
DATA = APP / "public/data/planner"
ASSETS_REV = "028c18f713baecad011301ff7a69acc39bcc2ae7"
ASSETS_URL = f"https://codeload.github.com/protomaps/basemaps-assets/zip/{ASSETS_REV}"
SPRITES_LICENSE_URL = "https://raw.githubusercontent.com/tangrams/icons/92510779634f4a006c61ea70e50cb8c52c765a81/LICENSE.md"
BW_BOUNDS = "7.45,47.5,10.5,49.85"


def bounds(value):
    try:
        west, south, east, north = map(float, value.split(","))
        if -180 <= west < east <= 180 and -85 <= south < north <= 85:
            return [west, south, east, north]
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("Use west,south,east,north in degrees.")


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def terrain_bounds(region):
    # Contours start at zoom 10 and read a 3×3 tile neighbourhood.
    count = 1 << 10
    west, south, east, north = region

    def tile_y(latitude):
        return (1 - math.asinh(math.tan(math.radians(latitude))) / math.pi) / 2 * count

    def latitude(y):
        return math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / count))))

    left = max(0, math.floor((west + 180) / 360 * count) - 1)
    right = min(count, math.floor((east + 180) / 360 * count) + 2)
    top = max(0, math.floor(tile_y(north)) - 1)
    bottom = min(count, math.floor(tile_y(south)) + 2)
    return [left / count * 360 - 180, latitude(bottom),
            right / count * 360 - 180, latitude(top)]


def install_assets(data, destination):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for entry in archive.infolist():
            parts = Path(entry.filename).parts[1:]
            if entry.is_dir() or not parts or ".." in parts:
                continue
            if parts[0] not in {"fonts", "sprites"}:
                continue
            path = destination.joinpath(*parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(archive.read(entry))
    for name in ["fonts/OFL.txt", "fonts/Noto Sans Regular/0-255.pbf",
                 "sprites/v4/light.json", "sprites/v4/dark@2x.png"]:
        if not (destination / name).is_file():
            raise ValueError(f"Map assets are missing {name}")


def verify_archive(pmtiles, path, tile_type, zoom):
    run(pmtiles, "verify", str(path))
    header = json.loads(run(pmtiles, "show", str(path), "--header-json",
                            capture_output=True, text=True).stdout)
    if header["tile_type"] != tile_type or header["minzoom"] != 0 or header["maxzoom"] != zoom:
        raise ValueError(f"{path.name} must contain {tile_type} tiles from zoom 0 to {zoom}")
    if tile_type == "mvt":
        metadata = json.loads(run(pmtiles, "show", str(path), "--metadata",
                                  capture_output=True, text=True).stdout)
        layers = {layer["id"] for layer in metadata.get("vector_layers", [])}
        if not {"earth", "water", "roads", "pois"} <= layers:
            raise ValueError("The basemap must use the Protomaps layer schema.")


def prepare(args):
    if DATA.exists():
        raise ValueError(f"{DATA} already exists. Move it aside before preparing another map.")
    DATA.parent.mkdir(parents=True, exist_ok=True)
    # Publish only a complete bundle. A failed download leaves the current map untouched.
    with tempfile.TemporaryDirectory(prefix=".planner-", dir=DATA.parent) as directory:
        stage = Path(directory)
        for name, source, kind, zoom in [("basemap", args.basemap, "mvt", 14),
                                         ("terrain", args.terrain, "webp", 12)]:
            path = stage / f"{name}.pmtiles"
            extract_bounds = terrain_bounds(args.bbox) if name == "terrain" else args.bbox
            run(args.pmtiles, "extract", source, str(path),
                "--bbox=" + ",".join(map(str, extract_bounds)), f"--maxzoom={zoom}")
            verify_archive(args.pmtiles, path, kind, zoom)
        with urlopen(ASSETS_URL, timeout=120) as response:
            install_assets(response.read(), stage / "assets")
        with urlopen(SPRITES_LICENSE_URL, timeout=30) as response:
            (stage / "assets/sprites/LICENSE.txt").write_bytes(response.read())
        manifest = {
            "bounds": args.bbox,
            "terrain_bounds": terrain_bounds(args.bbox),
            "sources": {"basemap": args.basemap, "terrain": args.terrain,
                        "assets": ASSETS_URL, "sprites_license": SPRITES_LICENSE_URL},
            "files": {},
        }
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                with path.open("rb") as stream:
                    digest = hashlib.file_digest(stream, "sha256").hexdigest()
                manifest["files"][path.relative_to(stage).as_posix()] = {
                    "bytes": path.stat().st_size, "sha256": digest,
                }
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        stage.rename(DATA)
    size = sum(item["bytes"] for item in manifest["files"].values())
    print(f"Prepared {DATA} ({size / 1024**2:.1f} MiB)")


def check_port(port):
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))


def serve(args):
    manifest = json.loads((DATA / "manifest.json").read_text())
    for name, item in manifest["files"].items():
        if (DATA / name).stat().st_size != item["bytes"]:
            raise ValueError(f"Incomplete map bundle: {name}")
    if args.port == args.tile_port:
        raise ValueError("The page and tile ports must differ.")
    check_port(args.port)
    check_port(args.tile_port)
    tile_origin = f"http://127.0.0.1:{args.tile_port}"
    env = {
        **os.environ,
        "OBC_PLANNER_TILES_URL": tile_origin,
        "OBC_PLANNER_ROUTING_URL": args.routing,
        "VITE_PLANNER_ROUTING_URL": "/routing",
        "VITE_PLANNER_PMTILES_URL": "/data/planner/basemap.pmtiles",
        "VITE_PLANNER_DEM_URL": "/tiles/terrain/{z}/{x}/{y}.webp",
        "VITE_PLANNER_GLYPHS_URL": "/data/planner/assets/fonts/{fontstack}/{range}.pbf",
        "VITE_PLANNER_SPRITES_URL": "/data/planner/assets/sprites/v4",
        "VITE_PLANNER_MAP_BOUNDS": ",".join(map(str, manifest["bounds"])),
    }
    children = []
    try:
        children.append(subprocess.Popen([
            args.pmtiles, "serve", str(DATA), "--interface=127.0.0.1",
            f"--port={args.tile_port}", f"--public-url={tile_origin}",
        ], start_new_session=True))
        children.append(subprocess.Popen([
            "npm", "run", "dev", "--", "--mode", "web", "--host", "127.0.0.1",
            "--port", str(args.port), "--strictPort",
        ], cwd=APP, env=env, start_new_session=True))
        print(f"Planner: http://127.0.0.1:{args.port}/planner.html", flush=True)
        print(f"Routing: {args.routing} (start route-server separately)", flush=True)
        while all(child.poll() is None for child in children):
            time.sleep(0.25)
        raise RuntimeError("A preview server stopped.")
    finally:
        for child in reversed(children):
            try:
                # npm starts Vite as a child; stop the complete process group.
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for child in children:
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pmtiles", default=os.environ.get("PMTILES", "pmtiles"))
    commands = parser.add_subparsers(dest="command", required=True)
    download = commands.add_parser("prepare", help="Extract Baden-Württemberg and copy map assets")
    download.add_argument("--basemap", required=True, help="Protomaps PMTiles source URL or file")
    download.add_argument("--terrain", default="https://download.mapterhorn.com/planet.pmtiles")
    download.add_argument("--bbox", type=bounds, default=BW_BOUNDS)
    preview = commands.add_parser("serve", help="Run the map tile server and planner")
    preview.add_argument("--port", type=int, default=4175)
    preview.add_argument("--tile-port", type=int, default=8789)
    preview.add_argument("--routing", default="http://127.0.0.1:8788")
    args = parser.parse_args()
    if not shutil.which(args.pmtiles):
        parser.error("Install the PMTiles CLI, or pass --pmtiles /path/to/pmtiles.")
    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        (prepare if args.command == "prepare" else serve)(args)
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner maps: {error}\n")


if __name__ == "__main__":
    main()
