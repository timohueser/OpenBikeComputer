"""The data registry for Python steps: the credit of a source and the box of a region.

specs/obc-data.md defines the files and `obc data` checks them. This module reads only what
Python steps need, so it adds no rule of its own. Shell scripts call it as

    python3 tools/data_registry.py box REGION [--lat-first]
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {source["id"]: source for source in tomllib.loads((ROOT / "data/sources.toml").read_text())["source"]}


def attribution(source, **fill):
    """The credit of a source as the product must show it. `fill` gives `{year}` and `{month}`."""
    return SOURCES[source]["attribution"].format(**fill)


def credit(source):
    """The credit and the licence of a source, `<attribution>; <licence>`."""
    return f"{attribution(source)}; {SOURCES[source]['licence']}"


def region_box(region):
    """[west, south, east, north] of a box region. A Python step reads box regions only."""
    document = tomllib.loads((ROOT / "data/regions" / f"{region}.toml").read_text())
    if document["kind"] != "box":
        raise ValueError(f"Region {region} is a {document['kind']} region. A Python step needs a box region.")
    return document["box"]


if __name__ == "__main__":
    if len(sys.argv) not in (3, 4) or sys.argv[1] != "box" or sys.argv[3:] not in ([], ["--lat-first"]):
        sys.exit("usage: data_registry.py box REGION [--lat-first]")
    west, south, east, north = region_box(sys.argv[2])
    print(",".join(map(str, [south, west, north, east] if sys.argv[3:] else [west, south, east, north])))
