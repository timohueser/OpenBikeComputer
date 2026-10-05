"""The data registry for Python steps: the credit of a source and the box of a region.

specs/obc-data.md defines the files and `obc data` checks them. This module reads only what
Python steps need, so it adds no rule of its own. Shell scripts call it as

    python3 tools/data_registry.py box REGION [--lat-first]
    python3 tools/data_registry.py attribution SOURCE
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {source["id"]: source for source in tomllib.loads((ROOT / "data/sources.toml").read_text())["source"]}


def attribution(source, **fill):
    """The credit of a source as the product must show it. `fill` gives `{year}` and `{month}`."""
    return SOURCES[source]["attribution"].format(**fill)


def credits(*sources):
    """The credits of several sources, as one line a rider reads."""
    return "; ".join(attribution(source) for source in sources)


def region(id):
    """The file of a region: `name`, `kind` and the key its kind names."""
    return tomllib.loads((ROOT / "data/regions" / f"{id}.toml").read_text())


def region_box(id):
    """[west, south, east, north] of a box region. A Python step reads box regions only."""
    document = region(id)
    if document["kind"] != "box":
        raise ValueError(f"Region {id} is a {document['kind']} region. A Python step needs a box region.")
    return document["box"]


if __name__ == "__main__":
    command, *rest = sys.argv[1:] or [""]
    if command == "attribution" and len(rest) == 1:
        print(attribution(rest[0]))
    elif command == "box" and rest[:1] and rest[1:] in ([], ["--lat-first"]):
        west, south, east, north = region_box(rest[0])
        print(",".join(map(str, [south, west, north, east] if rest[1:] else [west, south, east, north])))
    else:
        sys.exit("usage: data_registry.py box REGION [--lat-first] | attribution SOURCE")
