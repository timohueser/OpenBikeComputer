#!/usr/bin/env python3
"""Runs the planner routing step of `obc data` once, outside `obc data`, and measures it.

usage: bake_routing.py BINARY PBF GLO30_DIR OUT [--stop-at TEXT] [--bounds W S E N]
       [--countries CH,LI] [--region NAME]

BINARY is a release `obc-data` binary. The step reads PBF and the GLO-30 tiles of GLO30_DIR.
OUT must not exist: the script writes OUT/output (the layer), OUT/work (the bake folder),
OUT/stderr.log and OUT/result.json. With --stop-at, the script stops the bake when a line of its
standard error starts with TEXT, and removes the work folder.
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

PHASES = ["Importing", "Sampling terrain", "Preparing", "Writing the overlay index", "Writing the route catalog"]


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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("pbf")
    parser.add_argument("glo30")
    parser.add_argument("out")
    parser.add_argument("--stop-at")
    parser.add_argument("--bounds", nargs=4, type=float, default=[5.95, 45.81, 10.50, 47.81])
    parser.add_argument("--countries", default="CH")
    parser.add_argument("--region", default="switzerland")
    args = parser.parse_args()
    out = os.path.abspath(args.out)
    os.makedirs(out)
    output, work = os.path.join(out, "output"), os.path.join(out, "work")
    os.makedirs(output)
    os.makedirs(work)
    tiles = {name: os.path.join(os.path.abspath(args.glo30), name) for name in sorted(os.listdir(args.glo30))}
    request = {
        "step": "planner/routing",
        "snapshots": {"copernicus-glo-30": tiles},
        "layers": {"planner/osm/bounds": {"osm.pbf": os.path.abspath(args.pbf)}},
        "layer_files": {},
        "options": {
            "region": args.region,
            "bounds": args.bounds,
            "profiles": ["all"],
            "countries": args.countries.split(","),
            "attribution": "© OpenStreetMap contributors",
            "terrain_attribution": "Copernicus GLO-30",
        },
        "output": output,
        "metrics": os.path.join(out, "metrics.json"),
    }
    env = dict(os.environ, OBC_DATA_WORK=work, TMPDIR=work)
    started = time.monotonic()
    child = subprocess.Popen(
        [os.path.abspath(args.binary), "obc_data_step", "planner-router-build/routing"],
        stdin=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        cwd=out,
        text=True,
    )
    child.stdin.write(json.dumps(request))
    child.stdin.close()
    # Each phase: start time, peak RssAnon, peak work folder bytes.
    phases = []
    lines = []
    stopped = threading.Event()

    def read():
        with open(os.path.join(out, "stderr.log"), "w") as log:
            for line in child.stderr:
                log.write(line)
                log.flush()
                lines.append(line.rstrip("\n"))
                name = next((p for p in PHASES if line.startswith(p)), None)
                if line.startswith("Preparing profile"):
                    name = None
                if args.stop_at and line.startswith(args.stop_at):
                    stopped.set()
                    child.send_signal(signal.SIGTERM)
                    return
                if name:
                    phases.append({"phase": name, "start_s": round(time.monotonic() - started, 1), "rss_anon": 0, "work": 0})

    reader = threading.Thread(target=read)
    reader.start()
    tick = 0
    while child.poll() is None and not stopped.is_set():
        if phases:
            phases[-1]["rss_anon"] = max(phases[-1]["rss_anon"], rss_anon(child.pid))
            if tick % 4 == 0:
                phases[-1]["work"] = max(phases[-1]["work"], folder_bytes(work))
        tick += 1
        time.sleep(0.25)
    child.wait()
    reader.join()
    seconds = round(time.monotonic() - started, 1)
    result = {"seconds": seconds, "exit": child.returncode, "stopped": stopped.is_set(), "phases": phases}
    result["printed"] = [line for line in lines if line.startswith(("Imported:", "Manifest estimate"))]
    for i, phase in enumerate(phases):
        end = phases[i + 1]["start_s"] if i + 1 < len(phases) else seconds
        phase["seconds"] = round(end - phase["start_s"], 1)
        phase["rss_anon_mib"] = round(phase.pop("rss_anon") / 2**20)
        phase["work_mib"] = round(phase.pop("work") / 2**20)
    routing = os.path.join(output, "routing")
    if not stopped.is_set() and child.returncode == 0:
        result["sha256"] = {
            name: sha256(os.path.join(routing, name)) for name in ["manifest.json", "overlays.sqlite", "route-catalog.json"]
        }
        result["manifest_id"] = result["sha256"]["manifest.json"]
        result["manifest_bytes"] = os.path.getsize(os.path.join(routing, "manifest.json"))
        manifest = json.load(open(os.path.join(routing, "manifest.json")))
        result["roads"] = manifest["roads"]
        result["warnings"] = manifest["warnings"]
    shutil.rmtree(work, ignore_errors=True)
    with open(os.path.join(out, "result.json"), "w") as file:
        json.dump(result, file, indent=1)
    json.dump({k: v for k, v in result.items() if k != "warnings"}, sys.stdout, indent=1)
    print()
    return 0 if child.returncode == 0 or stopped.is_set() else 1


if __name__ == "__main__":
    sys.exit(main())
