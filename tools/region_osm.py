"""Combine area snapshots with prepared Osmium. No upstream state is inferred."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from tools import step_request


def digest(binary):
    with Path(binary).open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def probe(binary=None, expected=None):
    binary = binary or shutil.which(os.environ.get("OBC_OSMIUM", "osmium"))
    if binary is None:
        raise ValueError("Prepare Osmium, or set OBC_OSMIUM to its executable")
    binary = Path(binary).resolve()
    sha256 = digest(binary)
    if expected is not None and sha256 != expected:
        raise ValueError("Prepared Osmium changed; prepare a new plan")
    result = subprocess.run([binary, "--version"], capture_output=True, check=True)
    return binary, {"sha256": sha256, "version": result.stdout.decode().strip()}


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
    providers = [item for item in request.get("libraries", []) if item["name"] == "osmium"]
    if len(providers) != 1:
        raise ValueError("The request needs one named Osmium binding")
    provider = providers[0]
    path = Path(provider["path"])
    if not path.is_absolute() or path.resolve() != path:
        raise ValueError("Prepared Osmium path is not canonical; prepare a new plan")
    binary, identity = probe(path, provider["sha256"])
    inputs = []
    for files in request["layers"].values():
        selected = [Path(path) for name, path in files.items() if name.endswith(".osm.pbf")]
        if len(selected) != 1:
            raise ValueError("Each area input must contain one .osm.pbf")
        inputs.extend(selected)
    union(binary, inputs, Path(request["output"]) / "osm.pbf")
    if path.resolve() != path or digest(path) != provider["sha256"]:
        raise ValueError("Prepared Osmium changed during the union")
    step_request.metrics(request, {"areas": len(inputs), "osmium": identity["version"]})


if __name__ == "__main__":
    if "--probe" in __import__("sys").argv:
        print(json.dumps(probe()[1], sort_keys=True))
    else:
        step(step_request.read())
