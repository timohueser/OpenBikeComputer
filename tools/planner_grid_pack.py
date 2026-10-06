"""Pack planner routing, assets, model or joined fonts without rebuilding their inputs."""

import argparse
import json
from pathlib import Path

from . import planner_offline as offline, planner_runtime as runtime, step_request


def routing(layers):
    inputs = layers["planner/routing"]
    graph = json.loads(Path(inputs["blocks/blocks.json"]).read_bytes())
    catalog = json.loads(Path(inputs["blocks/catalog.json"]).read_bytes())
    if graph["source"] != catalog["source"]:
        raise ValueError("Routing grid sources differ")
    files = {"routing/blocks.json": inputs["blocks/blocks.json"]}
    for archive in graph["archives"]:
        for filename in ("pages.bin", "pages.idx"):
            name = f"packs/{archive}/{filename}"
            files[f"routing/{name}"] = inputs[f"blocks/{name}"]
    cells = []
    for cell in catalog["cells"]:
        name = f"offline/routing-cells/{cell['id']}.json"
        source = inputs[f"blocks/{cell['manifest']}"]
        descriptor = json.loads(Path(source).read_bytes())
        if descriptor["source"] != graph["source"] or descriptor["data"]["bounds"] != cell["bounds"]:
            raise ValueError("Routing cell source or coverage differs")
        files[name] = source
        cells.append({**cell, "manifest": name.removeprefix("offline/"), "sha256": runtime.digest(source)})
    for name, source in inputs.items():
        if name.startswith("routes/"):
            files[f"routes/tiles/{Path(name).name}"] = source
    return files, {"graph": graph, "cells": cells}


def pack_index(request, files, **extra):
    output = Path(request["output"])
    packed = {name: offline.pack_file(Path(source), output / "objects") for name, source in sorted(files.items())}
    (output / "index.json").write_bytes(runtime.encoded({"format": 1, "kind": request["options"]["kind"],
        "files": packed, **extra}))
    step_request.metrics(request, {"files": len(packed)})


def step(request):
    kind = request["options"]["kind"]
    if kind == "routing":
        files, extra = routing(request["layers"])
    else:
        layer, = request["layers"].values()
        prefix = {"assets": "maps/", "model": "search/"}[kind]
        files, extra = {prefix + name: source for name, source in layer.items()}, {}
    pack_index(request, files, **extra)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
