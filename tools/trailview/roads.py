"""The ways of the road solver of the TrailView feature tiles, `host/obc-trailview-features`: the roads, paths,
railways, tunnels and galleries that it sets the heights of, and their rules per class of
`data/trailview/roads.json`: the `highway` of a road, the `railway` of a track, or `rack` for a rack railway."""

import json
from pathlib import Path

from .structures import is_track

TABLE = Path(__file__).resolve().parents[2] / "data/trailview/roads.json"
FIELDS = {"grade", "smooth_m", "tol", "reach", "least", "cut", "fill", "tilt"}
KINDS = {"main-road", "road", "path", "railway", "tunnel", "gallery"}
# The classes of the tracks.
RAILS = {"rail", "light_rail", "narrow_gauge", "rack", "funicular"}
# A cable route on rock has no bench to stamp.
UNSOLVED = {"via_ferrata"}


def validate(classes):
    """The class table, or a ValueError that names the first row that breaks a rule. The table lists
    the classes from the most important down, as the junctions rank them; `path` and `road` are the
    rows of a way whose class the table lacks."""
    if not isinstance(classes, dict) or not {"road", "path"} <= classes.keys():
        raise ValueError("The road classes are an object with the rows road and path")
    for name, row in classes.items():
        number = lambda value: type(value) in (int, float) and value >= 0
        if not (isinstance(row, dict) and row.keys() == FIELDS and all(number(row[f]) for f in FIELDS - {"grade"})
                and (row["grade"] is None or number(row["grade"]) and 0 < row["grade"] < 1)):
            raise ValueError(f"Road class {name!r}: the fields are {sorted(FIELDS)}, numbers of 0 or more, and grade is null or below 1")
    return classes


CLASSES = validate(json.loads(TABLE.read_text()))


def solves(kind, tags):
    """Whether the road solver sets the heights of a feature."""
    return kind in KINDS and tags.get("highway") not in UNSOLVED


def class_of(tags):
    """The class of a way: `rack` for a rack railway, else the `railway` of a track or its `highway`."""
    if is_track(tags):
        return "rack" if tags.get("rack", "no") != "no" else tags["railway"]
    return tags.get("highway", "")


def rule_of(kind, tags, classes=CLASSES):
    """The row of the class table of a way: its class, else that of its kind."""
    name = class_of(tags)
    return name if name in classes else ("path" if kind == "path" else "road")


def layer(tags):
    """The OSM `layer` of a way, 0 where it has none."""
    try:
        return int(tags.get("layer", 0))
    except ValueError:
        return 0


def is_free(kind, tags):
    """Whether a way is a bridge or a tunnel: it ignores the terrain under it."""
    return kind == "tunnel" or tags.get("bridge", "no") != "no"
