"""Bake the Protomaps basemap from the prepared jar and store data."""

import argparse
import os
from pathlib import Path
import subprocess
import tempfile

from . import step_request


SOURCES = {
    "natural-earth": "natural_earth_vector.gpkg.zip",
    "water-polygons": "water-polygons-split-3857.zip",
    "land-polygons": "land-polygons-split-3857.zip",
    "daylight-landcover": "daylight-landcover.gpkg",
    "qrank": "qrank.csv.gz",
    "pgf-encoding": "pgf-encoding.zip",
}


def one(files, source):
    if len(files) != 1:
        raise ValueError(f"{source}: expected one input file")
    path, = files.values()
    path = Path(path).absolute()
    if not path.is_file():
        raise ValueError(f"{source}: missing input file {path}")
    return path


def step(request):
    output = Path(request["output"]).absolute()
    jar = one(step_request.files(request, "protomaps-basemaps"), "protomaps-basemaps")
    inputs = {name: one(step_request.files(request, source), source) for source, name in SOURCES.items()}
    osm = one(request["layers"]["planner/osm"], "planner/osm")
    # Upstream downloads QRank and PGF even without --download when their paths are absent.
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        (work / "data").mkdir()
        step_request.view(inputs, work / "data/sources")
        bounds = ",".join(map(str, request["options"]["bounds"]))
        subprocess.run(["java", "-Xmx6g", "-jar", str(jar), "--download=false", f"--osm-path={osm}",
                        f"--output={output / 'basemap.pmtiles'}", f"--bounds={bounds}",
                        "--maxzoom=14", f"--threads={os.cpu_count() or 1}",
                        "--attribution=" + request["options"]["attribution"]], cwd=work, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
