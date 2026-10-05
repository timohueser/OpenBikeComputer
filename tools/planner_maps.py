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
import threading
import time
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "builder/app"
# Child processes start in their own session, so an interrupt reaches only this process. A caller that
# runs producers in threads sets STOPPING and stops these; `run` then starts no new process.
RUNNING, STOPPING = set(), threading.Event()
# The versions live is built from. data/env/live.toml is their one home.
PINS = tomllib.loads((ROOT / "data/env/live.toml").read_text())["pins"]
ASSETS_REV = PINS["protomaps-assets"]
ASSETS_URL = f"https://codeload.github.com/protomaps/basemaps-assets/zip/{ASSETS_REV}"


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
        if STOPPING.is_set(): raise RuntimeError("The bake is stopping")
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


def mercator(lon, lat, zoom):
    """Fractional Web Mercator XYZ tile coordinates of a point."""
    count = 1 << zoom
    return (lon + 180) / 360 * count, (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * count


def tile_bounds(z, x, y):
    n = 1 << z
    latitude = lambda row: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * row / n))))
    return [x / n * 360 - 180, latitude(y + 1), (x + 1) / n * 360 - 180, latitude(y)]


def terrain_bounds(region):
    # Contours start at zoom 10 and read a 3×3 tile neighbourhood.
    count = 1 << 10
    left, top = (max(0, math.floor(value) - 1) for value in mercator(region[0], region[3], 10))
    right, bottom = (min(count - 1, math.floor(value) + 1) for value in mercator(region[2], region[1], 10))
    return tile_bounds(10, left, bottom)[:2] + tile_bounds(10, right, top)[2:]


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
        "python", "-m", "tools.planner_map_archive", source, destination,
        "--bbox=" + ",".join(map(str, region)), *(["--terrain"] if terrain else []),
        *([] if recompress else ["--no-recompress"]), cwd=ROOT)


def places_archive(pois, destination):
    run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
        "python", "-m", "tools.planner_places", pois, destination, cwd=ROOT)


def overlays_archive(index, destination):
    run("uv", "run", "--with-requirements", ROOT / "tools/requirements-planner-maps.txt",
        "python", "-m", "tools.planner_overlays", index, destination, cwd=ROOT)


def check_port(port):
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))


def check_bundle(folder, full=False):
    """The manifest of the map bundle in `folder`, once its files match it."""
    manifest = json.loads((folder / "manifest.json").read_text())
    for name, item in manifest["files"].items():
        path = folder / name
        if not path.resolve().is_relative_to(folder.resolve()):
            raise ValueError(f"Map path is outside the bundle: {name}")
        if path.stat().st_size != item["bytes"]:
            raise ValueError(f"Incomplete map bundle: {name}")
        if full:
            with path.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != item["sha256"]:
                    raise ValueError(f"Map checksum mismatch: {name}")
    return manifest


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
