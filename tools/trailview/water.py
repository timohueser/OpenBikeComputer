"""The waterways and lakes of the water solver of the TrailView feature tiles, `host/obc-trailview-features`:
what it sets the levels of, and the tags that it reads. specs/trailview-features.md says what the levels promise."""

import math
import re

import numpy as np

# The waterways and lakes that the solver sets, and the falls, weirs and dams that keep the smoothing of
# a level from crossing them.
RIVERS = {"river", "stream"}
LAKES = {"lake"}
MARKS = {"waterfall", "dam", "dam-point"}
# As WATERWAYS of the engine's bins.ts: the half-width of a waterway without a `width` tag.
HALF_WIDTHS = {"river": 5.0, "canal": 5.0, "stream": 1.0}
# Point and edge pairs that one numpy step handles at most.
PAIRS = 4_000_000


def solves(kind, tags):
    """Whether the solver reads or sets the heights of a feature: a river, a lake or what the solver only
    reads, but not a river in a tunnel."""
    return kind in RIVERS | LAKES | MARKS and not (kind in RIVERS and tags.get("tunnel", "no") != "no")


def half_width(tags):
    """The half-width of a waterway in metres: its `width`, else that of its kind."""
    width = number(tags.get("width"))
    return width / 2 if math.isfinite(width) else HALF_WIDTHS.get(tags.get("waterway"), 1.0)


def number(value):
    """The leading number of a tag, as JavaScript's parseFloat reads it: "8 m" is 8. NaN without one."""
    match = re.match(r"\s*[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?", value or "")
    return float(match.group()) if match else math.nan


def edges(rings):
    """The edges of the rings, as ax, ay, bx, by columns."""
    parts = [np.concatenate([r, np.roll(r, -1, 0)], 1) for r in (np.asarray(ring, np.float64) for ring in rings) if len(r) >= 3]
    return np.concatenate(parts) if parts else np.zeros((0, 4))


def contains(rings, x, y):
    """Per point, whether it lies inside the rings, even-odd."""
    shape, x, y = np.shape(x), np.ravel(x), np.ravel(y)
    ax, ay, bx, by = edges(rings).T
    inside = np.zeros(len(x), bool)
    step = max(1, PAIRS // max(len(ax), 1))
    for i in range(0, len(x), step):
        px, py = x[i:i + step, None], y[i:i + step, None]
        crosses = (ay > py) != (by > py)
        with np.errstate(all="ignore"):
            inside[i:i + step] = np.sum(crosses & (px < ax + (py - ay) * (bx - ax) / (by - ay)), 1) % 2 == 1
    return inside.reshape(shape)
