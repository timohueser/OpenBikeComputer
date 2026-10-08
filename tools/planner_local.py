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


APPS = {"web-planner": ("routing", "search", "tiles", "frontend"),
        "map-builder": ("tiles", "frontend"), "simulator": ("simulator",)}
PORTS = {"routing": 8788, "search": 8780, "tiles": 8789, "frontend": 5173}


def commands(value):
    root, view = Path(value["root"]), Path(value["view"])
    env = {**os.environ, "ROUTE_LISTEN": "127.0.0.1:8788", "OBC_SEARCH_PORT": "8780",
           "OBC_SEARCH_DATA": str(view / "data/search"), "OBC_SEARCH_REGIONS": value["region"],
           "OBC_SEARCH_PYTHON": sys.executable, "OBC_SEARCH_ORIGINS": "http://127.0.0.1:5173",
           "OBC_PLANNER_ROUTING_URL": "http://127.0.0.1:8788", "OBC_PLANNER_TILES_URL": "http://127.0.0.1:8789"}
    for key in ("NODE_OPTIONS", "NODE_PATH", "PYTHONPATH", "PYTHONHOME", "VITE_CATALOG_URL", "VITE_PLANNER_CONFIG"):
        env.pop(key, None)
    if value.get("release"):
        manifest = read(view / "planner" / "release.json")
        tiles = f"http://127.0.0.1:8789/releases/{value['release']}"
        files = manifest["files"]
        config = {**{key: manifest[key] for key in ("name", "bounds", "attribution", "landcover_attribution")},
                  "id": value["release"], "basemap": tiles + "/basemap.json", "places": tiles + "/places.json",
                  "overlays": tiles + "/overlays.json", "terrain": tiles + "/terrain/{z}/{x}/{y}.webp",
                  "terrain_attribution": read(view / "data/maps/terrain.json")["attribution"],
                  "glyphs": tiles + "/maps/assets/fonts/{fontstack}/{range}.pbf", "sprites": tiles + "/maps/assets/sprites/v4",
                  "routes": tiles + "/routes/tiles/{cell}.json", "routing": "/routing", "search": "/api/planner-search",
                  "layers": {name: tiles + f"/{name}.json" for name in runtime.DATA_LAYERS if f"maps/{name}.json" in files}}
        env["VITE_PLANNER_CONFIG"] = json.dumps(config)
    if value.get("maps_release"):
        env["VITE_CATALOG_URL"] = f"http://127.0.0.1:8789/cell-catalog/releases/{value['maps_release']}/catalog.json"
    node = value.get("node")
    return {
        "routing": ([str(view / "planner-service"), str(view / "data/routing")], root),
        "search": ([node, "server.mjs"], root / "planner/search"),
        "tiles": ([node, "src/local.mjs", str(view), "8789"], root / "planner/tiles"),
        "frontend": ([node, "node_modules/vite/bin/vite.js", "--mode", "web", "--host", "127.0.0.1",
                      "--port", "5173", "--strictPort"], root / "builder/web"),
        "simulator": ([str(view / "obc-sim"), str(view / "map.obcm"), "--physical"], root),
    }, env


def ready(value, app):
    def get(url):
        with urlopen(url, timeout=1) as response:
            return json.load(response)
    if app == "web-planner":
        route = get("http://127.0.0.1:8788/v1/region")
        search = get("http://127.0.0.1:8780/api/planner-search/status")
        expected = value["expected"]
        if route["package"] != expected["routing"] or not search["parser"]["ready"]:
            raise ValueError("Routing or search is not ready for the prepared data")
        if len(search["regions"]) != 1 or (search["regions"][0]["grid"] != expected["search"] or search["regions"][0]["id"] != value["region"]):
            raise ValueError("Search opened another prepared grid")
        if search["parser"]["model"] != expected["model"]:
            raise ValueError("Search opened another query model")
        with urlopen(f"http://127.0.0.1:8789/releases/{value['release']}/basemap.json", timeout=1): pass
    if app == "map-builder":
        with urlopen(f"http://127.0.0.1:8789/cell-catalog/releases/{value['maps_release']}/catalog.json", timeout=1): pass
    if app != "simulator":
        with urlopen("http://127.0.0.1:5173/" + ("planner.html" if app == "web-planner" else ""), timeout=1): pass


def check_view(value, apps):
    view = Path(value["view"])
    if "web-planner" in apps:
        manifest = runtime.release(view / "planner", include_sources=False)[1]
        for name, item in manifest["files"].items():
            if name.startswith(("routing/", "search/")) or name == "maps/terrain.json":
                offline.verify(view / "data" / name, item)
    for app, binary in (("web-planner", "planner-service"), ("simulator", "obc-sim")):
        if app in apps and runtime.digest(view / binary) != value["executables"][binary]:
            raise ValueError("Prepared native Local executable changed")
    if "simulator" in apps and runtime.digest(view / "map.obcm") != value["map_sha256"]:
        raise ValueError("Prepared Simulator map changed")


def supervise(directory, token, lock):
    directory = directory.resolve()
    children, current, intents, value = {}, None, {}, None
    apps = {app: {"status": "stopped", "message": None} for app in APPS}
    with lock.open("a") as owner:
        fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        def status(**values):
            desired = read(directory / "desired.json")
            if desired["token"] == token:
                write(directory / "state.json", {"token": token, "code": desired["code"], "apps": apps,
                                                  "region": value["region"] if value else None, "layers": value.get("layers", []) if value else [], **values})
        def required():
            return {child for app in intents if apps[app]["status"] != "failed" for child in APPS[app]}
        def drain_unused():
            needed = required()
            for name in reversed(list(children)):
                if name not in needed: maps.stop_process(children.pop(name))
        try:
            while True:
                stop = directory / "stop.json"
                if stop.exists() and read(stop).get("token") == token: break
                desired = read(directory / "desired.json")
                if desired["token"] != token: raise ValueError("Local owner token changed while apps run")
                wanted = desired["apps"]
                if not wanted or not set(wanted) <= APPS.keys(): raise ValueError("Local owner has no known requested app")
                value = read(Path(desired["view"]) / "service.json")
                if runtime.digest(Path(desired["view"]) / "service.json") != desired["sha256"]:
                    raise ValueError("Prepared service view changed")
                if current != value or wanted != intents:
                    for app in APPS:
                        if app not in wanted: apps[app] = {"status": "stopped", "message": None}
                        elif wanted[app] != intents.get(app): apps[app] = {"status": "starting", "message": None}
                    intents = wanted
                    for app in intents:
                        if apps[app]["status"] != "failed":
                            try: check_view(value, [app])
                            except (OSError, ValueError, KeyError) as error:
                                apps[app] = {"status": "failed", "message": str(error)}
                    drain_unused()
                    recipes, env = commands(value)
                    changed = [name for name in required() if name not in children or not current
                               or current["fingerprints"].get(name) != value["fingerprints"][name]]
                    for name in reversed(changed):
                        if name in children: maps.stop_process(children.pop(name))
                    for name in sorted(changed, key=lambda name: list(recipes).index(name)):
                        try:
                            if name in PORTS: maps.check_port(PORTS[name])
                            argv, cwd = recipes[name]
                            with (directory / f"{name}.log").open("ab") as logs:
                                children[name] = subprocess.Popen(argv, cwd=cwd, env=env, start_new_session=True, stdout=logs, stderr=logs)
                        except (OSError, ValueError) as error:
                            for app in intents:
                                if name in APPS[app]: apps[app] = {"status": "failed", "message": str(error)}
                    deadline = time.monotonic() + 90
                    while True:
                        for name, child in children.items():
                            if child.poll() is not None:
                                for app in intents:
                                    if name in APPS[app]: apps[app] = {"status": "failed", "message": f"Local {name} stopped; inspect its logs"}
                        drain_unused()
                        for app in intents:
                            if apps[app]["status"] != "failed":
                                try:
                                    ready(value, app)
                                    apps[app] = {"status": "ready", "message": None}
                                except (OSError, ValueError, KeyError) as error:
                                    apps[app] = {"status": "starting", "message": str(error)}
                                    if time.monotonic() >= deadline: apps[app]["status"] = "failed"
                        status(status="ready" if any(app["status"] == "ready" for app in apps.values()) else "starting", view=value["view"])
                        if stop.exists() and read(stop).get("token") == token: return
                        if read(directory / "desired.json") != desired: break
                        if all(apps[app]["status"] in ("ready", "failed") for app in intents): break
                        time.sleep(0.25)
                    current = value
                for name, child in children.items():
                    if child.poll() is not None:
                        for app in intents:
                            if name in APPS[app]: apps[app] = {"status": "failed", "message": f"Local {name} stopped; inspect its logs"}
                drain_unused()
                if all(apps[app]["status"] == "failed" for app in intents):
                    raise ValueError("All requested Local apps failed; inspect their app logs")
                status(status="ready", view=value["view"])
                time.sleep(0.25)
        except BaseException as error:
            status(status="failed", message=str(error))
            raise
        finally:
            for child in reversed(list(children.values())): maps.stop_process(child)
            if not (directory / "state.json").exists() or read(directory / "state.json").get("status") != "failed":
                for app in apps.values(): app["status"] = "stopped"
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
