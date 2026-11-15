"""Pack planner routing, assets, model or joined fonts without rebuilding their inputs."""

import argparse
from pathlib import Path

from . import planner_offline as offline, planner_runtime as runtime, step_request


def routing(layers):
    """The route catalog files of each grid cell."""
    inputs = layers["planner/routing"]
    return {f"routes/tiles/{Path(name).name}": source for name, source in inputs.items() if name.startswith("routes/")}


def pack_index(request, kind, files, **extra):
    output = Path(request["output"])
    packed = {name: offline.pack_file(Path(source), output / "objects") for name, source in sorted(files.items())}
    (output / "index.json").write_bytes(runtime.encoded({"format": 1, "kind": kind, "files": packed, **extra}))
    step_request.metrics(request, {"files": len(packed)})


def step(request):
    kind = request["options"]["kind"]
    if kind == "routing":
        files = routing(request["layers"])
    else:
        layer, = request["layers"].values()
        prefix = {"assets": "maps/", "model": "search/", "trailview-rules": "trailview/"}[kind]
        files = {prefix + name: source for name, source in layer.items()}
    pack_index(request, kind, files)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
