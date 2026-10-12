"""The subjects of the `wiki` source: OSM objects with a `wikidata` or `wikipedia` tag.

A subject line is `{"osm": "w10", "wikidata": "Q1", "wikipedia": "en:Castle", "peak": true,
"box": [west, south, east, north]}`; absent tags and an absent box are left out. `peak` marks a
`natural=peak` object: the producer selects those subjects for the peak compiler.
"""
from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess

from ..region_osm import probe, run
from . import catalog, dumps

LINK = re.compile(r"[a-z][a-z0-9-]{1,14}:.+")
# The OBCA grid: origin -2^28 µdeg on both axes; a shard cell is 2^20 µdeg. Cell indices are
# zero-padded like `obc_formats::grid::id_width`.
ORIGIN, CELL = 1 << 28, 20
WIDTH = max(4, len(str(((2 * ORIGIN) >> CELL) - 1)))
# A subject whose box touches more cells than this belongs to the common shard.
MAX_CELLS = 16


def extract(pbf: Path, out: Path, work: Path):
    """Osmium lists the tagged objects. `osmium export` assembles ways and areas for their boxes;
    an object that it gives no geometry, such as a route relation, has no box."""
    binary, _ = probe()
    work.mkdir(parents=True, exist_ok=True)
    filtered = work / "subjects.osm.pbf"
    run(binary, "tags-filter", pbf, "nwr/wikidata", "nwr/wikipedia", "--overwrite", "-o", filtered)
    boxes = {}
    for line in stream(binary, "export", filtered, "-f", "geojsonseq", "-a", "type,id", "-x", "print_record_separator=false"):
        feature = json.loads(line)
        tags = feature["properties"]
        if "wikidata" in tags or "wikipedia" in tags:
            west, south, east, north = 180.0, 90.0, -180.0, -90.0
            for lon, lat in coordinates(feature["geometry"]["coordinates"]):
                west, south, east, north = min(west, lon), min(south, lat), max(east, lon), max(north, lat)
            boxes[tags["@type"][0] + str(tags["@id"])] = [west, south, east, north]
    partial = out.with_suffix(".partial")
    with partial.open("w") as output:
        for line in stream(binary, "tags-filter", filtered, "nwr/wikidata", "nwr/wikipedia", "-R", "-f", "opl"):
            fields = line.split()
            tags = dict(part.split("=", 1) for part in next(field[1:] for field in fields if field.startswith("T")).split(",") if "=" in part)
            tags = {decoded(key): decoded(value) for key, value in tags.items()}
            subject = dict(osm=fields[0])
            if catalog.QID.fullmatch(tags.get("wikidata", "")):
                subject["wikidata"] = tags["wikidata"]
            if LINK.fullmatch(tags.get("wikipedia", "")):
                subject["wikipedia"] = tags["wikipedia"]
            if len(subject) == 1:
                continue
            if tags.get("natural") == "peak":
                subject["peak"] = True
            if fields[0] in boxes:
                subject["box"] = boxes[fields[0]]
            output.write(json.dumps(subject, ensure_ascii=False, separators=(",", ":")) + "\n")
    partial.replace(out)


def stream(binary, *args):
    with subprocess.Popen([binary, *map(str, args), "-o", "-"], stdout=subprocess.PIPE, text=True) as process:
        yield from process.stdout
    if process.returncode:
        raise ValueError(f"osmium {args[0]} failed")


def decoded(text: str) -> str:
    return re.sub(r"%([0-9a-fA-F]+)%", lambda match: chr(int(match[1], 16)), text)


def coordinates(value):
    if isinstance(value[0], (int, float)):
        yield value[:2]
    else:
        for item in value:
            yield from coordinates(item)


def cells(box: list[float] | None) -> list[str]:
    """The `<i>-<j>` cells that the box touches, or none when the subject belongs to common."""
    if box is None:
        return []
    south, west, north, east = ((round(value * 1e6) + ORIGIN) >> CELL for value in (box[1], box[0], box[3], box[2]))
    if (north - south + 1) * (east - west + 1) > MAX_CELLS:
        return []
    return [f"{i:0{WIDTH}}-{j:0{WIDTH}}" for i in range(south, north + 1) for j in range(west, east + 1)]


def load(db, work: Path, path: Path):
    def lines():
        with path.open("rb") as source:
            yield from dumps.records(source)

    def record(line: bytes):
        value = json.loads(line)
        db.executemany("INSERT OR REPLACE INTO subjects VALUES(?,?,?,?)",
                       [(value["osm"], cell, value.get("wikidata"), value.get("wikipedia")) for cell in cells(value.get("box")) or [""]])

    catalog.scan(db, work, "subjects", dumps.sha(path), lines(), record)
