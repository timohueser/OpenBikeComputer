"""Run the known Local planner children under one stable store owner."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import sysconfig
import time
from urllib.request import urlopen

from . import planner_maps as maps, planner_offline as offline, planner_runtime as runtime


def read(path):
    return json.loads(path.read_bytes())


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    offline.atomic_write(path, runtime.encoded(value))


def prepare(view):
    release = read(view / "planner" / "release.json")
    offline.materialize(view / "planner", view / "data", release, ("routing/", "search/", "maps/terrain.json"))
    return release


def commands(value):
    root, view = Path(value["root"]), Path(value["view"])
    manifest = read(view / "planner" / "release.json")
    tiles = f"http://127.0.0.1:8789/releases/{value['release']}"
    files = manifest["files"]
    config = {**{key: manifest[key] for key in ("name", "bounds", "attribution", "landcover_attribution")},
              "id": value["release"], "basemap": tiles + "/basemap.json", "places": tiles + "/places.json",
              "overlays": tiles + "/overlays.json", "terrain": tiles + "/terrain/{z}/{x}/{y}.webp",
              "terrain_attribution": read(view / "data/maps/terrain.json")["attribution"],
              "glyphs": tiles + "/maps/assets/fonts/{fontstack}/{range}.pbf",
              "sprites": tiles + "/maps/assets/sprites/v4", "routes": tiles + "/routes/tiles/{cell}.json",
              "routing": "/routing", "search": "/api/planner-search",
              "layers": {name: tiles + f"/{name}.json" for name in runtime.DATA_LAYERS if f"maps/{name}.json" in files}}
    env = {**os.environ, "ROUTE_LISTEN": "127.0.0.1:8788", "OBC_SEARCH_PORT": "8780",
           "OBC_SEARCH_DATA": str(view / "data/search"), "OBC_SEARCH_REGIONS": manifest["region"],
           "OBC_SEARCH_PYTHON": sys.executable, "OBC_SEARCH_ORIGINS": "http://127.0.0.1:5173", "VITE_PLANNER_CONFIG": json.dumps(config),
           "OBC_PLANNER_ROUTING_URL": "http://127.0.0.1:8788", "OBC_PLANNER_TILES_URL": "http://127.0.0.1:8789"}
    for key in ("NODE_OPTIONS", "NODE_PATH", "PYTHONPATH", "PYTHONHOME"):
        env.pop(key, None)
    node = value["node"]
    return {
        "routing": ([str(view / "planner-service"), str(view / "data/routing")], root),
        "search": ([node, "server.mjs"], root / "planner/search"),
        "tiles": ([node, "src/local.mjs", str(view), "8789"], root / "planner/tiles"),
        "frontend": ([node, "node_modules/vite/bin/vite.js", "--mode", "web", "--host", "127.0.0.1",
                      "--port", "5173", "--strictPort"], root / "builder/web"),
    }, env


def ready(value):
    def get(url):
        with urlopen(url, timeout=1) as response:
            return json.load(response)
    route = get("http://127.0.0.1:8788/v1/region")
    search = get("http://127.0.0.1:8780/api/planner-search/status")
    expected = value["expected"]
    if route["package"] != expected["routing"] or not search["parser"]["ready"]:
        raise ValueError("Routing or search is not ready for the prepared data")
    if len(search["regions"]) != 1 or (search["regions"][0]["grid"] != expected["search"]
                                       or search["regions"][0]["id"] != value["region"]):
        raise ValueError("Search opened another prepared grid")
    if search["parser"]["model"] != expected["model"]:
        raise ValueError("Search opened another query model")
    with urlopen(f"http://127.0.0.1:8789/releases/{value['release']}/basemap.json", timeout=1):
        pass
    with urlopen("http://127.0.0.1:5173/planner.html", timeout=1):
        pass


def supervise(directory, token, lock):
    directory = directory.resolve()
    children, current = {}, None
    with lock.open("a") as owner:
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            desired = read(directory / "desired.json")
            if desired["token"] == token:
                write(directory / "state.json", {"token": token, "code": desired["code"], "status": "failed",
                                                  "message": "Another Local supervisor still owns the services"})
            raise
        def status(**values):
            if read(directory / "desired.json")["token"] == token:
                write(directory / "state.json", {"token": token, "code": read(directory / "desired.json")["code"], **values})
        try:
            while True:
                stop = directory / "stop.json"
                if stop.exists() and read(stop).get("token") == token:
                    break
                desired = read(directory / "desired.json")
                if desired["token"] != token:
                    raise ValueError("Local owner token changed while services run")
                value = read(Path(desired["view"]) / "service.json")
                if runtime.digest(Path(desired["view"]) / "service.json") != desired["sha256"]:
                    raise ValueError("Prepared service view changed")
                if current != value:
                    status(status="starting", view=value["view"])
                    manifest = runtime.release(Path(value["view"]) / "planner", include_sources=False)[1]
                    for name, item in manifest["files"].items():
                        if name.startswith(("routing/", "search/")) or name == "maps/terrain.json":
                            offline.verify(Path(value["view"]) / "data" / name, item)
                    if runtime.digest(Path(value["view"]) / "planner-service") != value["routing_executable"]:
                        raise ValueError("Prepared native route service changed")
                    recipes, env = commands(value)
                    changed = [name for name in recipes if not current
                               or current["fingerprints"][name] != value["fingerprints"][name]]
                    for name in reversed(changed):
                        if name in children:
                            maps.stop_process(children.pop(name))
                    if not current:
                        for port in (5173, 8780, 8788, 8789): maps.check_port(port)
                    for name in changed:
                        argv, cwd = recipes[name]
                        children[name] = subprocess.Popen(argv, cwd=cwd, env=env, start_new_session=True)
                    deadline = time.monotonic() + 90
                    while True:
                        if any(child.poll() is not None for child in children.values()):
                            raise ValueError("A Local planner service stopped before readiness")
                        if stop.exists() and read(stop).get("token") == token:
                            return
                        try:
                            ready(value)
                            break
                        except (OSError, ValueError, KeyError):
                            if time.monotonic() >= deadline: raise
                            time.sleep(0.25)
                    current = value
                    status(status="ready", view=value["view"], url="http://127.0.0.1:5173/planner.html")
                if any(child.poll() is not None for child in children.values()):
                    raise ValueError("A Local planner service stopped")
                time.sleep(0.25)
        except BaseException as error:
            status(status="failed", message=str(error))
            raise
        finally:
            for child in reversed(list(children.values())):
                maps.stop_process(child)
            if not (directory / "state.json").exists() or read(directory / "state.json").get("status") != "failed":
                status(status="stopped")
            write(directory / "drained.json", {"token": token})


def check_python(base, expected):
    actual = {"implementation": sys.implementation.name, "version": list(sys.version_info[:3]),
              "abi": sysconfig.get_config_var("SOABI")}
    digest = hashlib.sha256(json.dumps(actual, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    if Path(sys._base_executable).resolve(strict=True) != base.resolve(strict=True) or digest != expected:
        raise ValueError("Prepare the Local Python environment with the selected interpreter")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare", type=Path)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--token")
    parser.add_argument("--lock", type=Path)
    parser.add_argument("--base-python", type=Path)
    parser.add_argument("--python-runtime")
    args = parser.parse_args()
    if args.prepare:
        prepare(args.prepare)
        return
    if not args.directory or not args.token or not args.lock or not args.base_python or not args.python_runtime:
        parser.error("Provide one Local owner directory, token, lock and selected interpreter")
    check_python(args.base_python, args.python_runtime)
    def stop(_number, _frame):
        raise KeyboardInterrupt("Local services interrupted")
    signal.signal(signal.SIGTERM, stop)
    supervise(args.directory, args.token, args.lock)


if __name__ == "__main__":
    main()
