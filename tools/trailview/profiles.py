"""The master terrain for the solvers of the TrailView feature tiles, and simple water levels from it: a
river lies at the terrain smoothed along it and held from rising downstream, and a lake at the median of
the terrain inside it. The water solver replaces them; until then the stamps of the engine have levels to
shape the terrain to. `roads.py` solves the roads."""

from collections import OrderedDict
import io
import json
import math
import os
from pathlib import Path
import sqlite3

import numpy as np

RIVERS = {"river", "stream"}
LAKES = {"lake"}
# As GRADE_SMOOTH_M of the prototype's grading: a river follows the terrain averaged over ±12 m.
SMOOTH_M = 12.0
STEP_M = 2.0
# A lake reads the terrain at no more than this many points.
LAKE_POINTS = 250_000
EQUATOR_M = 40075016.686
TILE = 512
CACHED = 256


def solves(kind, tags):
    """Whether this module sets the heights of a feature: a river that is not in a tunnel, or a lake."""
    return kind in LAKES or (kind in RIVERS and tags.get("tunnel", "no") == "no")


class Terrain:
    """The master terrain: Terrarium WebP tiles of 512 px, R × 256 + G + B / 256 − 32768 metres, unknown
    unless opaque, from `read(z, x, y)` up to zoom `top`. A point reads the deepest zoom that knows its
    four pixels."""

    def __init__(self, read, top):
        self.read, self.top = read, top
        self.cache = OrderedDict()

    @classmethod
    def mbtiles(cls, path):
        """The terrain of a `planner-dem` MBTiles file."""
        db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        query = "SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?"
        read = lambda z, x, y: (row := db.execute(query, (z, x, (1 << z) - 1 - y)).fetchone()) and row[0]
        return cls(read, db.execute("SELECT max(zoom_level) FROM tiles").fetchone()[0])

    @classmethod
    def grid(cls, files):
        """The terrain of the grid of `planner/terrain` ({path: object}): a tile is in the archive of its
        zoom-11 tile, a tile of zoom 13 in the archive of `13/`, and a tile of a lower zoom in its own."""
        from pmtiles.reader import Reader

        index = json.loads(Path(files["index.json"]).read_bytes())
        prefix, readers = "maps/tiles/terrain/", {}
        archives = {name[len(prefix):-len(".pmtiles")]: Path(files[f"objects/{entry['sha256']}"])
                    for name, entry in index["files"].items() if name.startswith(prefix) and name.endswith(".pmtiles")}

        def read(z, x, y):
            level = min(z, 11)
            part = f"{level}-{x >> (z - level)}-{y >> (z - level)}"
            part = f"13/{part}" if z == 13 else part
            if part not in archives:
                return None
            if part not in readers:
                stream = archives[part].open("rb")
                readers[part] = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset))
            return readers[part].get(z, x, y)

        return cls(read, 13 if any(part.startswith("13/") for part in archives) else 12)

    def tile(self, z, x, y):
        key = (z, x, y)
        if key in self.cache:
            self.cache.move_to_end(key)
            return self.cache[key]
        from PIL import Image

        data = self.read(z, x, y)
        heights = None
        if data:
            rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"), np.float32)
            heights = rgba[..., 0] * 256 + rgba[..., 1] + rgba[..., 2] / 256 - 32768
            heights[rgba[..., 3] < 255] = np.nan
        self.cache[key] = heights
        if len(self.cache) > CACHED:
            self.cache.popitem(last=False)
        return heights

    def pixels(self, z, i, j):
        """The heights of the pixels (i, j) of zoom z, counted across the world; NaN where unknown."""
        out = np.full(i.shape, np.nan, np.float32)
        tiles = (i // TILE) * (1 << z) + j // TILE
        for key in np.unique(tiles):
            heights = self.tile(z, int(key // (1 << z)), int(key % (1 << z)))
            if heights is not None:
                at = tiles == key
                out[at] = heights[j[at] % TILE, i[at] % TILE]
        return out

    def heights(self, x, y):
        """Bilinear heights at world coordinates in [0, 1], y down; NaN where no zoom knows them."""
        out = np.full(np.shape(x), np.nan, np.float32)
        todo = np.arange(out.size)
        for z in range(self.top, -1, -1):
            if not todo.size:
                break
            n = (1 << z) * TILE
            u, v = x[todo] * n - 0.5, y[todo] * n - 0.5
            i, j = np.floor(u).astype(np.int64), np.floor(v).astype(np.int64)
            fu, fv = (u - i).astype(np.float32), (v - j).astype(np.float32)
            i0, i1, j0, j1 = np.clip(i, 0, n - 1), np.clip(i + 1, 0, n - 1), np.clip(j, 0, n - 1), np.clip(j + 1, 0, n - 1)
            h = ((self.pixels(z, i0, j0) * (1 - fu) + self.pixels(z, i1, j0) * fu) * (1 - fv)
                 + (self.pixels(z, i0, j1) * (1 - fu) + self.pixels(z, i1, j1) * fu) * fv)
            known = np.isfinite(h)
            out[todo[known]] = h[known]
            todo = todo[~known]
        return out


def metres(y):
    """Metres per world unit at world row y: the Mercator scale."""
    return EQUATOR_M * np.cos(np.arctan(np.sinh(np.pi * (1 - 2 * y))))


def along(points, terrain):
    """The heights at the vertices of a line: the terrain sampled every STEP_M, and a straight line fitted
    to it over ±SMOOTH_M of length along the line. Toward the ends the window narrows to one side, and the
    line keeps a constant grade unbiased. NaN where the terrain is unknown over the whole window."""
    xy = np.asarray(points, np.float64)
    if len(xy) < 2:
        return np.full(len(xy), np.nan)
    d = np.diff(xy, axis=0)
    lengths = np.hypot(*d.T) * metres((xy[:-1, 1] + xy[1:, 1]) / 2)
    steps = np.maximum(1, np.ceil(lengths / STEP_M)).astype(int)
    t = np.concatenate([np.arange(n) / n for n in steps] + [[1.0]])
    segment = np.concatenate([np.full(n, k) for k, n in enumerate(steps)] + [[len(d) - 1]])
    samples = xy[segment] + t[:, None] * d[segment]
    along = np.concatenate([[0], np.cumsum(lengths)])[segment] + t * lengths[segment]
    h = terrain.heights(samples[:, 0], samples[:, 1]).astype(np.float64)
    known = np.isfinite(h)
    s0 = np.where(known, along - along[-1] / 2, 0)
    sums = [np.concatenate([[0], np.cumsum(value)]) for value in (known, s0, s0 * s0, np.where(known, h, 0), s0 * np.where(known, h, 0))]
    lo = np.searchsorted(along, along - SMOOTH_M)
    hi = np.searchsorted(along, along + SMOOTH_M, side="right")
    n, sx, sxx, sz, sxz = (total[hi] - total[lo] for total in sums)
    spread = n * sxx - sx * sx
    slope = np.where(spread > 1e-9 * np.maximum(n, 1) ** 2, (n * sxz - sx * sz) / np.where(spread > 0, spread, 1), 0)
    fit = np.where(n > 0, (sz - slope * sx) / np.maximum(n, 1) + slope * (along - along[-1] / 2), np.nan)
    vertices = np.concatenate([[0], np.cumsum(steps)])
    return fit[vertices]


def river(points, terrain):
    """The water level at the vertices of a river, which OSM draws in the direction of its flow: the
    terrain smoothed as for a road, never rising downstream."""
    return np.fmin.accumulate(along(points, terrain))


def inside(rings, xs, ys):
    """Per point of the grid xs × ys, whether it lies inside the rings, even-odd: a hole is a ring too."""
    edges = np.array([(*a, *b) for ring in rings for a, b in zip(ring, ring[1:] + ring[:1])], np.float64)
    ax, ay, bx, by = edges.T
    y = ys[:, None]
    crosses = (ay <= y) != (by <= y)
    x = np.where(crosses, ax + (y - ay) * (bx - ax) / np.where(by == ay, 1, by - ay), np.inf)
    x.sort(axis=1)
    counts = np.array([np.searchsorted(row, xs) for row in x])
    return counts % 2 == 1


def lake(parts, terrain):
    """The median of the terrain inside an area, NaN where it knows none. A lake smaller than the grid
    reads the mean of its vertices."""
    rings = [points for _, points, *_ in parts]
    xy = np.concatenate([np.asarray(ring, np.float64) for ring in rings])
    (x0, y0), (x1, y1) = xy.min(0), xy.max(0)
    area = sum(abs(sum(ax * by - bx * ay for (ax, ay), (bx, by) in zip(ring, ring[1:] + ring[:1]))) / 2 for ring in rings)
    step = max(1 / ((1 << terrain.top) * TILE), math.sqrt(area / LAKE_POINTS))
    xs, ys = np.arange(x0 + step / 2, x1, step), np.arange(y0 + step / 2, y1, step)
    mask = inside(rings, xs, ys) if xs.size and ys.size else np.zeros((0, 0), bool)
    if mask.any():
        rows, columns = np.nonzero(mask)
        h = terrain.heights(xs[columns], ys[rows])
    else:
        h = terrain.heights(*np.asarray(rings[0], np.float64).mean(0, keepdims=True).T)
    return float(np.nanmedian(h)) if np.isfinite(h).any() else math.nan


def solve(features, terrain):
    """Per feature (kind, tags, parts), the levels of each part, None where unknown."""
    out = []
    for kind, _, parts in features:
        if kind in LAKES:
            level = lake(parts, terrain)
            out.append([[level] * len(points) for _, points, *_ in parts])
        else:
            out.append([river(points, terrain).tolist() for _, points, *_ in parts])
    return [[[h if math.isfinite(h) else None for h in z] for z in heights] for heights in out]
