#!/usr/bin/env python3
"""Runs the four planner routing steps of `obc data` (from #3299) outside `obc data`, and measures them.

usage: bake_layers.py BINARY PBF GLO30_DIR OUT [--layers DIR] [--profiles all] [--kill-at TEXT]
       [--bounds W S E N] [--countries CH,LI] [--region NAME] [--cell-sizes 512,...] [--download-level 4]

The steps are planner/routing-source, -roads, -costs and planner/routing (the package). Each work
layer goes to LAYERS/<name>-<key>/output, LAYERS defaults to OUT/layers. As the engine does, a step
whose key (its options and the keys of its inputs) has a complete layer in LAYERS is reused; pass the
LAYERS of an earlier run to reuse its layers. The package goes to OUT/output. With --kill-at, the
script kills the bake (SIGKILL) when a line of its standard error starts with TEXT: the step that
runs leaves no complete layer. OUT must not exist. OUT/result.json has the time, the peak RssAnon,
the peak work folder and the layer size of each step, and the SHA-256 of the three files of the
gate. From #3301 the package step also gets `cell_sizes` and `download_level`, and `result.json` has
`cells`: the SHA-256 of `cells/directory.bin` (the identity of format 10), the file count and bytes.
"""
import argparse
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def folder_bytes(path):
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(os.path.join(root, name)).st_blocks * 512
            except FileNotFoundError:
                pass
    return total


def rss_anon(pid):
    try:
        with open(f"/proc/{pid}/status") as status:
            for line in status:
                if line.startswith("RssAnon:"):
                    return int(line.split()[1]) * 1024
    except FileNotFoundError:
        pass
    return 0


def files_of(folder):
    """Each file below `folder`, by its path in it."""
    found = {}
    for root, _, names in os.walk(folder):
        for name in names:
            path = os.path.join(root, name)
            found[os.path.relpath(path, folder)] = path
    return found


def run(binary, function, request, out, kill_at, lines):
    """Runs one step. Returns (exit, killed, seconds, peak RssAnon, peak work bytes)."""
    work = os.path.join(out, "work")
    os.makedirs(work, exist_ok=True)
    env = dict(os.environ, OBC_DATA_WORK=work, TMPDIR=work)
    started = time.monotonic()
    child = subprocess.Popen(
        [os.path.abspath(binary), "obc_data_step", function],
        stdin=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=out, text=True,
    )
    child.stdin.write(json.dumps(request))
    child.stdin.close()
    killed = threading.Event()

    def read():
        with open(os.path.join(out, "stderr.log"), "a") as log:
            for line in child.stderr:
                log.write(line)
                log.flush()
                lines.append(line.rstrip("\n"))
                if kill_at and line.startswith(kill_at) and not killed.is_set():
                    killed.set()
                    child.send_signal(signal.SIGKILL)

    reader = threading.Thread(target=read)
    reader.start()
    peak, disk, tick = 0, 0, 0
    while child.poll() is None:
        peak = max(peak, rss_anon(child.pid))
        if tick % 4 == 0:
            disk = max(disk, folder_bytes(work))
        tick += 1
        time.sleep(0.25)
    child.wait()
    reader.join()
    shutil.rmtree(work, ignore_errors=True)
    return child.returncode, killed.is_set(), round(time.monotonic() - started, 1), peak, disk


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("pbf")
    parser.add_argument("glo30")
    parser.add_argument("out")
    parser.add_argument("--layers")
    parser.add_argument("--profiles", default="all")
    parser.add_argument("--kill-at")
    parser.add_argument("--bounds", nargs=4, type=float, default=[5.95, 45.81, 10.50, 47.81])
    parser.add_argument("--countries", default="CH")
    parser.add_argument("--region", default="switzerland")
    parser.add_argument("--cell-sizes", default="512,4096,32768,262144,2097152,16777216,134217728")
    parser.add_argument("--download-level", type=int, default=4)
    args = parser.parse_args()
    out = os.path.abspath(args.out)
    os.makedirs(out)
    layers = os.path.abspath(args.layers or os.path.join(out, "layers"))
    os.makedirs(layers, exist_ok=True)
    pbf = os.path.abspath(args.pbf)
    tiles = {name: os.path.join(os.path.abspath(args.glo30), name) for name in sorted(os.listdir(args.glo30))}
    cut = "planner/osm/bounds"
    pbf_sha = sha256(pbf)
    keys = {cut: pbf_sha}
    folders = {cut: None}
    attribution = "© OpenStreetMap contributors"
    sizes = [int(size) for size in args.cell_sizes.split(",")]
    steps = [
        ("planner/routing-source", [cut], {"bounds": args.bounds}, False),
        ("planner/routing-roads", ["planner/routing-source"],
         {"bounds": args.bounds, "terrain_attribution": "Copernicus GLO-30",
          "cell_sizes": sizes}, True),
        ("planner/routing-costs", ["planner/routing-source", "planner/routing-roads"],
         {"bounds": args.bounds, "profiles": args.profiles.split(",")}, False),
        ("planner/routing", ["planner/routing-source", "planner/routing-roads", "planner/routing-costs"],
         {"region": args.region, "bounds": args.bounds, "countries": args.countries.split(","),
          "attribution": attribution, "cell_sizes": sizes, "download_level": args.download_level}, False),
    ]
    result = {"steps": [], "layers": layers}
    lines = []
    started = time.monotonic()
    for name, inputs, options, terrain in steps:
        spec = {"step": name, "options": options, "inputs": [keys[i] for i in inputs]}
        if terrain:
            spec["tiles"] = sorted(tiles)
        key = hashlib.sha256(json.dumps(spec, sort_keys=True).encode()).hexdigest()[:16]
        keys[name] = key
        package = name == "planner/routing"
        folder = os.path.join(out, "output") if package else os.path.join(layers, f"{name.rsplit('-')[-1]}-{key}")
        folders[name] = folder
        record = {"step": name, "key": key}
        if not package and os.path.exists(os.path.join(folder, "done")):
            record["reused"] = True
            record["layer_mib"] = round(folder_bytes(os.path.join(folder, "output")) / 2**20)
            result["steps"].append(record)
            print(f"{name}: reused {folder}", file=sys.stderr)
            continue
        shutil.rmtree(folder, ignore_errors=True)
        output = folder if package else os.path.join(folder, "output")
        os.makedirs(output)
        request_layers, layer_files = {}, {}
        for i in inputs:
            if i == cut:
                request_layers[cut] = {"osm.pbf": pbf}
                layer_files[cut] = [{"path": "osm.pbf", "size": os.path.getsize(pbf), "sha256": pbf_sha}]
            else:
                request_layers[i] = files_of(os.path.join(folders[i], "output"))
        request = {
            "step": name,
            "snapshots": {"copernicus-glo-30": tiles} if terrain else {},
            "layers": request_layers,
            "layer_files": layer_files,
            "options": options,
            "output": output,
            "metrics": os.path.join(out, "metrics.json"),
        }
        function = "planner-router-build/" + name.split("/", 1)[1]
        print(f"{name}: baking", file=sys.stderr)
        code, killed, seconds, peak, disk = run(args.binary, function, request, out, args.kill_at, lines)
        record.update(seconds=seconds, rss_anon_mib=round(peak / 2**20), work_mib=round(disk / 2**20), exit=code)
        if killed:
            record["killed"] = True
        result["steps"].append(record)
        if code != 0:
            break
        if not package:
            open(os.path.join(folder, "done"), "w").close()
            record["layer_mib"] = round(folder_bytes(output) / 2**20)
    result["seconds"] = round(time.monotonic() - started, 1)
    result["stages"] = [line for line in lines if line.startswith(("Stage ", "Imported:", "Manifest estimate"))]
    routing = os.path.join(out, "output", "routing")
    if result["steps"][-1].get("exit") == 0 and result["steps"][-1]["step"] == "planner/routing":
        result["sha256"] = {
            name: sha256(os.path.join(routing, name)) for name in ["manifest.json", "overlays.sqlite", "route-catalog.json"]
        }
        manifest = json.load(open(os.path.join(routing, "manifest.json")))
        result["manifest_bytes"] = os.path.getsize(os.path.join(routing, "manifest.json"))
        result["roads"] = manifest["roads"]
        cells = os.path.join(out, "output", "cells")
        if os.path.exists(os.path.join(cells, "directory.bin")):
            names = os.listdir(cells)
            result["cells"] = {"identity": sha256(os.path.join(cells, "directory.bin")), "files": len(names),
                               "bytes": sum(os.path.getsize(os.path.join(cells, name)) for name in names)}
    with open(os.path.join(out, "result.json"), "w") as file:
        json.dump(result, file, indent=1)
    json.dump({k: v for k, v in result.items() if k != "stages"}, sys.stdout, indent=1)
    print()
    return 0 if "sha256" in result or args.kill_at else 1


if __name__ == "__main__":
    sys.exit(main())
