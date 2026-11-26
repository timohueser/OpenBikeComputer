#!/usr/bin/env python3
"""PROTOTYPE for GitHub issue 3836: puts the route pairs into the compare page.

    build_compare.py PAIRS.json OUT.html

PAIRS.json is `{"sets": [...]}` in the layout that `compare.html` describes.
"""
import json
import pathlib
import sys

pairs, out = sys.argv[1:3]
data = json.loads(pathlib.Path(pairs).read_text())
for chosen in data["sets"]:
    for pair in chosen["pairs"]:
        for route in pair["routes"].values():
            route["line"] = [[round(lat, 5), round(lon, 5)] for lat, lon in route["line"]]
page = (pathlib.Path(__file__).parent / "compare.html").read_text()
head, _, tail = page.split("/*DATA*/")
text = json.dumps(data, separators=(",", ":")).replace("</", "<\\/")
pathlib.Path(out).write_text(f"{head}const DATA = {text};{tail}")
print(f"{out}: {sum(len(s['pairs']) for s in data['sets'])} pairs in {len(data['sets'])} sets")
