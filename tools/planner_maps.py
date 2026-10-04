#!/usr/bin/env python3
"""Planner map archive helpers, and a command that compacts an archive to a smaller box."""

import argparse
import hashlib
import io
import json
import math
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import zipfile

try:
    from .planner_runtime import DATA_LAYERS
except ImportError:
    from planner_runtime import DATA_LAYERS

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "builder/app"
DATA = None  # the maps folder of the data directory in use; its caller sets it
# Child processes start in their own session, so an interrupt reaches only this process. A caller that
# runs producers in threads stops these when it stops.
RUNNING = set()
ASSETS_REV = "028c18f713baecad011301ff7a69acc39bcc2ae7"
ASSETS_URL = f"https://codeload.github.com/protomaps/basemaps-assets/zip/{ASSETS_REV}"
SPRITES_LICENSE_URL = "https://raw.githubusercontent.com/tangrams/icons/92510779634f4a006c61ea70e50cb8c52c765a81/LICENSE.md"


def bounds(value):
    try:
        west, south, east, north = map(float, value.split(","))
        if -180 <= west < east <= 180 and -85 <= south < north <= 85:
            return [west, south, east, north]
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("Use west,south,east,north in degrees.")


def run(*args, **kwargs):
    input_data = kwargs.pop("input", None)
    if input_data is not None: kwargs["stdin"] = subprocess.PIPE
    if kwargs.pop("capture_output", False):
        kwargs.update(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    process = subprocess.Popen([str(arg) for arg in args], start_new_session=True, **kwargs)
    RUNNING.add(process)
    try:
        stdout, stderr = process.communicate(input_data)
    except BaseException:
        stop_process(process)
        raise
    finally:
        RUNNING.discard(process)
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, args, stdout, stderr)
    return subprocess.CompletedProcess(args, process.returncode, stdout, stderr)


def stop_running():
    for process in list(RUNNING):
        stop_process(process)


def stop_process(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


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


def compact_archive(source, destination, region, terrain=False, recompress=True):
    run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
        "python", ROOT / "tools/planner_map_archive.py", source, destination,
        "--bbox=" + ",".join(map(str, region)), *(["--terrain"] if terrain else []),
        *([] if recompress else ["--no-recompress"]), cwd=ROOT)


def places_archive(basemap, destination):
    run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
        "python", "-m", "tools.planner_places", basemap, destination, cwd=ROOT)


def overlays_archive(index, destination):
    run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
        "python", "-m", "tools.planner_overlays", index, destination, cwd=ROOT)


def check_port(port):
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))


def check_bundle(full=False):
    manifest = json.loads((DATA / "manifest.json").read_text())
    for name, item in manifest["files"].items():
        path = DATA / name
        if not path.resolve().is_relative_to(DATA.resolve()):
            raise ValueError(f"Map path is outside the bundle: {name}")
        if path.stat().st_size != item["bytes"]:
            raise ValueError(f"Incomplete map bundle: {name}")
        if full:
            with path.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != item["sha256"]:
                    raise ValueError(f"Map checksum mismatch: {name}")
    return manifest


def preview(args):
    manifest = check_bundle()
    tile_origin = f"http://127.0.0.1:{args.tile_port}"
    base = "/@fs" + str(DATA.resolve())
    env = {
        **os.environ,
        "OBC_PLANNER_MAPS_DIR": str(DATA.resolve()),
        "OBC_PLANNER_TILES_URL": tile_origin,
        "OBC_PLANNER_ROUTING_URL": args.routing,
        "VITE_PLANNER_ROUTING_URL": "/routing",
        "VITE_PLANNER_PMTILES_URL": base + "/basemap.pmtiles",
        "VITE_PLANNER_PLACES_URL": base + "/places.pmtiles",
        "VITE_PLANNER_OVERLAYS_URL": base + "/overlays.pmtiles",
        # Without its archive, the planner offers no such data layer.
        **{f"VITE_PLANNER_{layer.upper()}_URL": f"{base}/{layer}.pmtiles" if f"{layer}.pmtiles" in manifest["files"] else ""
           for layer in DATA_LAYERS},
        "VITE_PLANNER_DEM_URL": "/tiles/terrain/{z}/{x}/{y}.webp",
        "VITE_PLANNER_GLYPHS_URL": base + "/assets/fonts/{fontstack}/{range}.pbf",
        "VITE_PLANNER_SPRITES_URL": base + "/assets/sprites/v4",
        "VITE_PLANNER_MAP_BOUNDS": ",".join(map(str, manifest["bounds"])),
    }
    commands = [
        ([args.pmtiles, "serve", str(DATA), "--interface=127.0.0.1",
          f"--port={args.tile_port}", f"--public-url={tile_origin}"], ROOT),
        (["npm", "run", "dev", "--", "--mode", "web", "--host", "127.0.0.1",
          "--port", str(args.port), "--strictPort"], APP),
    ]
    return commands, env


def supervise(commands, env, ready=None):
    children = []
    try:
        for command, cwd in commands:
            children.append(subprocess.Popen(command, cwd=cwd, env=env, start_new_session=True))
        if ready:
            ready(children)
        while all(child.poll() is None for child in children):
            time.sleep(0.25)
        raise RuntimeError("A local planner service stopped.")
    finally:
        for child in reversed(children):
            # npm starts Vite as a child; stop the complete process group.
            stop_process(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    compact = commands.add_parser("compact", help="Extract a box and losslessly compress its terrain")
    compact.add_argument("source", type=Path)
    compact.add_argument("output", type=Path)
    compact.add_argument("--bbox", type=bounds, required=True)
    compact.add_argument("--terrain", action="store_true")
    compact.add_argument("--no-recompress", action="store_true")
    args = parser.parse_args()
    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        compact_archive(args.source, args.output, args.bbox, args.terrain, not args.no_recompress)
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner maps: {error}\n")


if __name__ == "__main__":
    main()
