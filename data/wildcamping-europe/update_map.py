#!/usr/bin/env python3
"""Embed the rulebook in the standalone map."""

import argparse
import json
import re
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--check", action="store_true", help="Check without writing the map")
args = parser.parse_args()
root = Path(__file__).resolve().parent
data = json.loads((root / "data.json").read_text(encoding="utf-8"))
embedded = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
path = root / "map.html"
html = path.read_text(encoding="utf-8")
pattern = r'(<script id="research-data" type="application/json">)(.*?)(</script>)'
matches = list(re.finditer(pattern, html, re.DOTALL))
if len(matches) != 1:
    raise SystemExit("Expected one research-data block in map.html")
match = matches[0]
if args.check:
    if match.group(2) != embedded:
        raise SystemExit("Map data differs from data.json; run update_map.py")
    print("Map data matches data.json")
else:
    updated = html[:match.start(2)] + embedded + html[match.end(2):]
    path.write_text(updated, encoding="utf-8")
    print("Updated map data")
