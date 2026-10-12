"""Combine area snapshots with prepared Osmium. No upstream state is inferred."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from tools import step_request


def probe():
    binary = shutil.which(os.environ.get("OBC_OSMIUM", "osmium"))
    if binary is None:
        raise ValueError("Prepare Osmium, or set OBC_OSMIUM to its executable")
    result = subprocess.run([binary, "--version"], capture_output=True, check=True)
    return Path(binary).resolve(), result.stdout.decode().strip()


def run(binary, *args):
    result = subprocess.run([binary, *args], capture_output=True, check=False)
    if result.returncode:
        raise ValueError(f"Prepared Osmium failed: {result.stderr.decode(errors='replace')}")
    return result.stdout


def union(binary, inputs, output):
    if not inputs:
        raise ValueError("No area extracts selected")
    with tempfile.TemporaryDirectory(dir=output.parent.parent) as temporary:
        history = Path(temporary) / "union.osh.pbf"
        run(binary, "merge", "--with-history", "-F", "pbf", "--generator=obc-region-osm", "-o", history, *inputs)
        info = json.loads(run(binary, "fileinfo", "--extended", "--json", history))
        timestamp = info["data"]["timestamp"]["last"] or "1970-01-01T00:00:01Z"
        run(binary, "time-filter", history, timestamp, "--generator=obc-region-osm", "-o", output)
        run(binary, "check-refs", "-F", "pbf", output)


def step(request):
    binary, version = probe()
    inputs = []
    for files in request["layers"].values():
        selected = [Path(path) for name, path in files.items() if name.endswith(".osm.pbf")]
        if len(selected) != 1:
            raise ValueError("Each area input must contain one .osm.pbf")
        inputs.extend(selected)
    union(binary, inputs, Path(request["output"]) / "osm.pbf")
    step_request.metrics(request, {"areas": len(inputs), "osmium": version})


if __name__ == "__main__":
    step(step_request.read())
