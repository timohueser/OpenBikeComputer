#!/usr/bin/env python3
"""PROTOTYPE for GitHub issue 3836: puts the route pairs into the compare page.

    build_compare.py PAIRS.json OUT.html

PAIRS.json is `{"sets": [...]}` in the layout that `compare.html` describes.
"""
import json
import pathlib
import sys

import math

# About 3 m: a point closer than this to the line between its kept neighbours is left out.
TOLERANCE_DEG = 0.00003


def simplified(line):
    """Douglas-Peucker without recursion, in degrees with the longitude scaled to the latitude."""
    scale = math.cos(math.radians(line[0][0]))
    points = [(lat, lon * scale) for lat, lon in line]
    keep = [False] * len(points)
    keep[0] = keep[-1] = True
    stack = [(0, len(points) - 1)]
    while stack:
        first, last = stack.pop()
        (ay, ax), (by, bx) = points[first], points[last]
        length = math.hypot(by - ay, bx - ax)
        worst, at = 0.0, None
        for k in range(first + 1, last):
            py, px = points[k]
            if length == 0.0:
                distance = math.hypot(py - ay, px - ax)
            else:
                distance = abs((by - ay) * (px - ax) - (bx - ax) * (py - ay)) / length
            if distance > worst:
                worst, at = distance, k
        if at is not None and worst > TOLERANCE_DEG:
            keep[at] = True
            stack += [(first, at), (at, last)]
    return [[round(lat, 5), round(lon, 5)] for (lat, lon), kept in zip(line, keep) if kept]


pairs, out = sys.argv[1:3]
data = json.loads(pathlib.Path(pairs).read_text())
for chosen in data["sets"]:
    for pair in chosen["pairs"]:
        for route in pair["routes"].values():
            route["line"] = simplified(route["line"])
page = (pathlib.Path(__file__).parent / "compare.html").read_text()
head, _, tail = page.split("/*DATA*/")
text = json.dumps(data, separators=(",", ":")).replace("</", "<\\/")
pathlib.Path(out).write_text(f"{head}const DATA = {text};{tail}")
print(f"{out}: {sum(len(s['pairs']) for s in data['sets'])} pairs in {len(data['sets'])} sets")
