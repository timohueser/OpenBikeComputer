"""Prototype: joins the sets of route pairs of each region into one file.

Usage: python3 -I join_pairs.py OUT.json PART.json...
"""

import json
import sys

sets = []
for path in sys.argv[2:]:
    with open(path) as part:
        sets.extend(json.load(part))
with open(sys.argv[1], "w") as out:
    json.dump({"sets": sets}, out, separators=(",", ":"))
print("Wrote", sys.argv[1], [(one["id"], len(one["pairs"])) for one in sets])
