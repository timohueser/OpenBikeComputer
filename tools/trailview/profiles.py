"""The master terrain of the TrailView feature tiles, for the lifts, the bridges and the tracks. The solvers of
`solvers.py` read it themselves."""

from collections import OrderedDict
import io
import json
import os
from pathlib import Path
import sqlite3
import threading

import numpy as np

EQUATOR_M = 40075016.686
TILE = 512
CACHED = 256


class Terrain:
    """The master terrain: Terrarium WebP tiles of 512 px, R × 256 + G + B / 256 − 32768 metres, unknown
    unless opaque, from `read(z, x, y)` up to zoom `top`. A point reads the deepest zoom that knows its
    four pixels. The workers of a step share one: as forked processes, or as threads where a fork is not safe. `origin`
    names the file or the files of the terrain for the solvers of `solvers.py`."""

    def __init__(self, read, top, origin=None):
        self.read, self.top, self.origin = read, top, origin
        self.cache, self.lock = OrderedDict(), threading.Lock()

    @classmethod
    def mbtiles(cls, path):
        """The terrain of a `planner-dem` MBTiles file. Each process and thread opens its own connection: SQLite
        shares one neither across a fork nor between threads."""
        local = threading.local()

        def db():
            if getattr(local, "pid", None) != os.getpid():
                local.db, local.pid = sqlite3.connect(f"file:{path}?mode=ro", uri=True), os.getpid()
            return local.db

        query = "SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?"
        read = lambda z, x, y: (row := db().execute(query, (z, x, (1 << z) - 1 - y)).fetchone()) and row[0]
        return cls(read, db().execute("SELECT max(zoom_level) FROM tiles").fetchone()[0], {"mbtiles": str(path)})

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

        top = 13 if any(part.startswith("13/") for part in archives) else 12
        return cls(read, top, {"grid": {name: str(path) for name, path in files.items()}})

    def tile(self, z, x, y):
        key = (z, x, y)
        with self.lock:
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
        with self.lock:
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
