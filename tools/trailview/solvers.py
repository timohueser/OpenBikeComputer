"""The solvers of the heights of the TrailView feature tiles: the levels of rivers and lakes and the heights of roads,
which `host/obc-trailview-features` sets from the master terrain. This module writes its job file, runs it
and reads the heights back; `job.rs` there defines both files."""

import json
import math
import os
from pathlib import Path
import struct
import subprocess
import tempfile

import numpy as np

from . import roads, structures, water
from .. import step_pool

ROOT = Path(__file__).resolve().parents[2]
WATER_KINDS = {**{kind: 0 for kind in water.RIVERS}, **{kind: 1 for kind in water.LAKES}, **{kind: 2 for kind in water.MARKS}}


def points(out, xy):
    xy = np.asarray(xy, "<f8").reshape(-1, 2)
    out += struct.pack("<I", len(xy)) + xy.tobytes()


def road(out, kind, tags, parts, classes):
    """Append a road feature (kind, tags, parts) to the bytes of a job."""
    free, tunnel = roads.is_free(kind, tags), kind == "tunnel"
    layer = roads.layer(tags)
    flags = free | tunnel << 1 | (tags.get("ford", "no") != "no") << 2 | ("layer" in tags) << 3
    flags |= (not free and layer < 0 and tags.get("tunnel", "no") == "no") << 4
    rule = list(classes).index(roads.rule_of(kind, tags, classes))
    out += struct.pack("<BIqdI", flags, rule, layer, structures.road_half(tags), len(parts))
    for _, xy, *_ in parts:
        points(out, xy)


class Past:
    """The road features past the region that a chain of bridges or tunnels may need, as the bytes of a job: a
    fraction of the memory of the features."""

    def __init__(self, classes=roads.CLASSES):
        self.bytes, self.count, self.classes = bytearray(), 0, classes

    def add(self, kind, tags, parts):
        road(self.bytes, kind, tags, parts, self.classes)
        self.count += 1


def lake_level(tags):
    """How a lake finds its level: 0 from its terrain, 1 a pond, 2 a reservoir."""
    return 1 if tags.get("water") == "pond" else 2 if "reservoir" in (tags.get("water"), tags.get("landuse")) else 0


def binary():
    """The solver program: OBC_TRAILVIEW_HEIGHTS, else the release build of the workspace."""
    if program := os.environ.get("OBC_TRAILVIEW_HEIGHTS"):
        return program
    command = ["cargo", "build", "--release", "--locked", "--quiet", "--message-format=json", "-p", "obc-trailview-features",
               "--bin", "trailview-heights", "--manifest-path", str(ROOT / "Cargo.toml")]
    messages = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True).stdout.splitlines()
    return next(m["executable"] for m in map(json.loads, messages) if m.get("reason") == "compiler-artifact" and m.get("executable"))


def solve(waters, ways, beyond, inside, terrain, classes=roads.CLASSES):
    """The heights of the features (kind, tags, parts): per feature of `waters` the levels of its parts, and per
    feature of `ways` its parts with their heights, or the parts as they are where the input cuts its chain of
    bridges or tunnels. `beyond` is the `Past` of the region, `inside` the box x0, y0, x1, y1 past which the input
    cuts a chain. A waterway that OSM draws uphill turns round in place. Also the
    culverts and the tunnels under a road of `ways`, and the metrics of the water and of the roads."""
    names = list(classes)
    header = json.dumps({
        "terrain": terrain.origin, "path": names.index("path"), "inside": list(inside), "memory": step_pool.free_memory(),
        "classes": [{"grade": row["grade"], "smooth_m": row["smooth_m"], "rail": name in roads.RAILS} for name, row in classes.items()],
    }).encode()
    job = bytearray(struct.pack("<Q", len(header)) + header)
    job += struct.pack("<Q", len(waters))
    for kind, tags, parts in waters:
        job += struct.pack("<BBddI", WATER_KINDS[kind], lake_level(tags), water.half_width(tags), water.number(tags.get("ele")), len(parts))
        for outer, xy, *_ in parts:
            job += struct.pack("<B", outer)
            points(job, xy)
    job += struct.pack("<QQ", len(ways), beyond.count)
    for kind, tags, parts in ways:
        road(job, kind, tags, parts, classes)
    job += beyond.bytes
    with tempfile.TemporaryDirectory(prefix=".trailview-heights-") as work:
        path, out = Path(work) / "job", Path(work) / "heights"
        path.write_bytes(job)
        del job
        subprocess.run([binary(), path, out], check=True)
        data = memoryview(out.read_bytes())
    length = struct.unpack_from("<Q", data)[0]
    found = json.loads(bytes(data[8:8 + length]))
    at = 8 + length
    unknown = lambda z: [None if math.isnan(v) else v for v in z.tolist()]
    levels = []
    for _, _, parts in waters:
        heights = []
        for _, xy, *_ in parts:
            turned, count = struct.unpack_from("<Bq", data, at)
            at += 9
            if turned:
                xy.reverse()
            heights.append(None if count < 0 else unknown(np.frombuffer(data, "<f8", count, at)))
            at += 8 * max(count, 0)
        levels.append(heights)
    solved = []
    for _, _, parts in ways:
        out = []
        for outer, xy, z in parts:
            count = struct.unpack_from("<q", data, at)[0]
            at += 8
            if count < 0:
                out.append((outer, xy, z))
                continue
            vertices = np.frombuffer(data, "<f8", 3 * count, at).reshape(-1, 3)
            at += 24 * count
            out.append((outer, list(map(tuple, vertices[:, :2].tolist())), unknown(vertices[:, 2])))
        solved.append(out)
    metrics = found["roads"]
    metrics["grade_excess_m"] = round(metrics["grade_excess_m"], 4)
    metrics["grade_excess_at"] = [round(value, 5) for value in metrics["grade_excess_at"]]
    metrics["residual_p99_m"] = round(metrics["residual_p99_m"], 2)
    return levels, solved, set(found["culverts"]), set(found["underpasses"]), found["water"], metrics
