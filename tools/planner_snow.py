"""The planner snow archives of `specs/planner-snow-tiles.md`.

`prepare` writes the files of one zoom 6 block of the prepared source `snow` (specs/obc-data.md, `snow`), one
season at a time:

    python -m tools.planner_snow prepare --block X-Y --seasons 2016,2017 --modis LIST --canopy FILE,… --out DIR

MODIS comes from the files of the archive dataset `modis-snow` that LIST names, one path a line: the Planetary
Computer COG of the NDSI band of a granule, or the NSIDC HDF4 granule where the Planetary Computer has none. HR-WSI
comes from the Copernicus Data Space S3 endpoint `https://eodata.dataspace.copernicus.eu`, with
`CDSE_S3_ACCESS_KEY` and `CDSE_S3_SECRET_KEY` in `~/.config/openbikecomputer/cdse-s3.env`.

The `obc data` step `planner/snow` (`--step`) joins the seasons of the blocks of a region into `snow.pmtiles`,
`snow-levels.pmtiles` and `snow-seasons.pmtiles`. `box --bounds W,S,E,N --output DIR` bakes the HR-WSI seasons of a
box without MODIS into the three archives in DIR; the TrailView test box uses it.
"""

import argparse
from collections import deque, namedtuple
import contextlib
import datetime as dt
import functools
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

import numpy as np

from . import planner_geo as geo, step_pool, step_request
from .trailview.tiles import lossless

NO_SNOW, FULL, NO_DATA = 253, 254, 255
LAST_STEP = 182
TILE = 256
# `snow_format` of the three archives.
FORMAT = 1
# Band b holds the day steps 4b to 4b + 3.
BANDS = 46
# The levels of the runs, in the order of the levels table: whether a level holds a band at a pixel with k seasons
# of data, c of them with snow on the day of the band. A pixel with k = 0 has no data at each level.
LEVELS = {
    "any": lambda c, k: c >= 1,
    "quarter": lambda c, k: (c >= 1) & (4 * c >= k),
    "half": lambda c, k: (c >= 1) & (2 * c >= k),
    "three quarters": lambda c, k: 4 * c >= 3 * k,
    "every": lambda c, k: c == k,
}
# `snow` holds the run of the half level; `snow-levels` the others, in the order of LEVELS.
HALF = list(LEVELS).index("half")
# The pixels a side of a `snow-seasons` tile.
SEASON_TILE = 64
# The archives of `join`, and whether their tiles are WebP images.
ARCHIVES = {"snow": True, "snow-levels": True, "snow-seasons": False}
# The zoom of a block, of the chunks that a worker makes, and the max zoom of each source.
BLOCK_ZOOM, CHUNK_ZOOM, HR_ZOOM, MODIS_ZOOM = 6, 10, 12, 8
RESOLUTION_M = {"nasa-modis": 500, "copernicus-hr-wsi": 20}
# MODIS sees the canopy, not the snow under it; HR-WSI corrects for trees. The owner chose 75 % canopy cover as
# "dense": below it, MODIS still sees the snow between the trees.
DENSE_CANOPY_PERCENT = 75
ODATA = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products"
COPERNICUS_PRODUCT = "CLMS_WSI_SP_020m_"
CDSE_KEYS = Path.home() / ".config/openbikecomputer/cdse-s3.env"
USER_AGENT = "OpenBikeComputer planner snow bake (https://github.com/timohueser/OpenBikeComputer)"
# The MODIS sinusoidal grid: 36 × 18 tiles of 2400 × 2400 pixels.
SINUSOIDAL = "+proj=sinu +lon_0=0 +x_0=0 +y_0=0 +R=6371007.181 +units=m +no_defs"
MODIS_TILE_M, MODIS_PIXELS = 1111950.5197665233, 2400
MODIS_X0, MODIS_Y0 = -20015109.355798, 10007554.677899
MODIS_SNOW_NDSI = 10
MODIS_FILE = re.compile(r"(MOD|MYD)10A1\.A(\d{4})(\d{3})\.h(\d\d)v(\d\d)\.061\.\d+(_NDSI_Snow_Cover\.tif|\.hdf)")
# A worker of a chunk took at most 259 MB for nine seasons of HR-WSI at 47° N (central Switzerland); it makes one
# season. The 8 programs of a prepare run at once, so each takes few workers.
WORKERS = 2
# The connections to CDSE S3 that a user may hold at once. The programs of a prepare share them through lock files.
CDSE_CONNECTIONS = 4

Grid = namedtuple("Grid", "crs transform shape")


def season_days(season):
    """The days of `season`, from 1 September."""
    return (dt.date(season + 1, 9, 1) - dt.date(season, 9, 1)).days


def encode(onset, length, days, data):
    """Onset and melt-out bytes (2, ...) of the longest snow period: its first day index and its length."""
    meltout = onset + length - 1
    dated = np.stack([np.clip(onset, 0, 2 * LAST_STEP) // 2, np.clip(meltout, 0, 2 * LAST_STEP) // 2])
    value = np.where(length >= days, FULL, np.where(length == 0, NO_SNOW, dated))
    return np.where(data, value, NO_DATA).astype(np.uint8)


class SnowSeason:
    """The longest snow period of one season of `days` days, from daily observations fed one day at a time.

    A day without a clear observation takes the state of the nearest clear day of the season; on a tie, the
    earlier one.
    """

    def __init__(self, days, shape):
        self.days = days
        self.day = -1
        self.last = np.full(shape, -1, np.int32)  # day of the last clear observation
        self.snow = np.zeros(shape, bool)  # snow on that day
        self.start = np.zeros(shape, np.int32)  # first day of the open snow period
        self.clear = np.zeros(shape, bool)
        self.length = np.zeros(shape, np.int32)
        self.onset = np.zeros(shape, np.int32)

    def observe(self, day, clear, snow):
        if not self.day < day < self.days:
            raise ValueError("Days must increase and stay inside the season")
        self.day = day
        self.clear |= clear
        snow = clear & snow
        switch = (self.last + day) // 2 + 1  # the first gap day nearer to today than to the last clear day
        self._credit(clear & self.snow & ~snow, self.start, switch - 1)
        self.start = np.where(snow & ~self.snow, np.where(self.last < 0, 0, switch), self.start)
        self.last = np.where(clear, day, self.last)
        self.snow = np.where(clear, snow, self.snow)

    def _credit(self, mask, first, last):
        """Offer the snow period from `first` to `last` (inclusive)."""
        longer = mask & (last - first + 1 > self.length)
        self.length = np.where(longer, last - first + 1, self.length)
        self.onset = np.where(longer, first, self.onset)

    def planes(self):
        """The planes (1, 2, ...) of the season."""
        self._credit(self.snow, self.start, np.full_like(self.start, self.days - 1))
        return encode(self.onset, self.length, self.days, self.clear)[None]


def blend(values, weights, median=False):
    """The spec's blend rule: bytes (inputs, seasons, 2, N) with weights (inputs, N) -> (seasons, 2, N).

    With `median`, dated pixels take the median of the dated inputs instead of the weighted mean.
    """
    onset = values[:, :, 0]
    w = weights[:, None, :]
    share = lambda mask: (w * mask).sum(0)
    dated_mask = onset <= LAST_STEP
    dated, full, none, missing = share(dated_mask), share(onset == FULL), share(onset == NO_SNOW), share(onset == NO_DATA)
    winner = np.argmax(np.stack([dated, full, none]), axis=0)[:, None]
    dw = (w * dated_mask)[:, :, None]
    if median:
        # Undated inputs sort last, so the k dated inputs of a pixel come first. A pixel without one takes 0.
        ordered = np.sort(np.where(dated_mask[:, :, None], values, NO_DATA), axis=0)
        count = dated_mask.sum(0)[:, None]
        low, high = (np.take_along_axis(ordered, index[None], 0)[0] for index in (np.maximum(count - 1, 0) // 2, count // 2))
        centre = np.where(count > 0, (low.astype(np.float32) + high) / 2, 0)
    else:
        centre = (dw * values).sum(0) / np.maximum(dated, 1e-9)[:, None]
    out = np.where(winner == 0, np.floor(centre + 0.5), np.where(winner == 1, FULL, NO_SNOW))
    return np.where((missing > weights.sum(0) / 2)[:, None], NO_DATA, out).astype(np.uint8)


def tile_lonlat(z, x, y):
    """Longitude and latitude of the 256 × 256 pixel centres of tile z/x/y, row order from the north."""
    n = 2 ** z * TILE
    j, i = np.meshgrid(np.arange(TILE) + 0.5, np.arange(TILE) + 0.5)
    lon = (x * TILE + j) / n * 360 - 180
    lat = np.degrees(np.arctan(np.sinh(np.pi * (1 - 2 * (y * TILE + i) / n))))
    return lon.ravel(), lat.ravel()


def sample(planes, grid, z, x, y):
    """Bilinear blend of the source planes (seasons, 2, rows, cols) at the pixel centres of a tile."""
    from rasterio.warp import transform

    lon, lat = tile_lonlat(z, x, y)
    xs, ys = transform("EPSG:4326", grid.crs, lon, lat)
    col, row = ~grid.transform @ (np.asarray(xs), np.asarray(ys))
    col, row = col - 0.5, row - 0.5
    c0, r0 = np.floor(col).astype(np.int64), np.floor(row).astype(np.int64)
    fc, fr = col - c0, row - r0
    rows, cols = grid.shape
    values, weights = [], []
    for dr, dc, weight in ((0, 0, (1 - fr) * (1 - fc)), (0, 1, (1 - fr) * fc), (1, 0, fr * (1 - fc)), (1, 1, fr * fc)):
        r, c = r0 + dr, c0 + dc
        inside = (r >= 0) & (r < rows) & (c >= 0) & (c < cols)
        value = planes[:, :, np.clip(r, 0, rows - 1), np.clip(c, 0, cols - 1)]
        values.append(np.where(inside, value, NO_DATA))
        weights.append(weight)
    return blend(np.stack(values), np.stack(weights).astype(np.float32)).reshape(planes.shape[0], 2, TILE, TILE)


def smooth(tile):
    """Each pixel from its 3 × 3 neighbourhood in the tile: the majority class and the median day."""
    seasons = tile.shape[0]
    padded = np.pad(tile, ((0, 0), (0, 0), (1, 1), (1, 1)), mode="edge")
    values = np.stack([padded[:, :, r:r + TILE, c:c + TILE] for r in range(3) for c in range(3)])
    weights = np.full((9, TILE * TILE), 1 / 9, np.float32)
    return blend(values.reshape(9, seasons, 2, TILE * TILE), weights, median=True).reshape(tile.shape)


def parent(children, seasons, smoothed=False):
    """A tile from its four children {(dx, dy): planes or None}, each pixel blended from 2 × 2 children."""
    half = TILE // 2
    out = np.full((seasons, 2, TILE, TILE), NO_DATA, np.uint8)
    for (dx, dy), child in children.items():
        if child is None:
            continue
        if smoothed:
            child = smooth(child)
        values = child.reshape(seasons, 2, half, 2, half, 2).transpose(3, 5, 0, 1, 2, 4).reshape(4, seasons, 2, half * half)
        block = blend(values, np.full((4, half * half), 0.25, np.float32))
        out[:, :, dy * half:(dy + 1) * half, dx * half:(dx + 1) * half] = block.reshape(seasons, 2, half, half)
    return out


def children(z, x, y):
    """The four children of tile z/x/y as ((dx, dy), (z + 1, x', y'))."""
    return [((dx, dy), (z + 1, 2 * x + dx, 2 * y + dy)) for dy in (0, 1) for dx in (0, 1)]


def day_step(band):
    """The day step of band `band`: the third of its four, or the last of the season."""
    return min(4 * band + 2, LAST_STEP)


def runs(planes):
    """The first and the last band (levels, 2, ...) of the run of each of LEVELS at each pixel of the per-year planes
    (seasons, 2, ...): its longest run of consecutive bands, the earliest on a tie. 253 where the level holds no band,
    255 where no season has data."""
    onset, meltout = planes[:, 0], planes[:, 1]
    known = (onset != NO_DATA).sum(0, dtype=np.int16)
    dated, full = onset <= LAST_STEP, onset == FULL
    # The run that ends at the band, the longest run and its last band.
    run, best, end = (np.zeros((len(LEVELS), *onset.shape[1:]), np.uint8) for _ in range(3))
    for band in range(BANDS):
        day = day_step(band)
        snowy = (dated & (onset <= day) & (day <= meltout) | full).sum(0, dtype=np.int16)
        run += 1
        run *= np.stack([holds(snowy, known) for holds in LEVELS.values()])
        # Only a longer run replaces the best, so the earliest one wins on a tie.
        end[run > best] = band
        np.maximum(best, run, out=best)
    found = np.where((best == 0)[:, None], NO_SNOW, np.stack([end - best + 1, end], 1))
    return np.where(known == 0, NO_DATA, found).astype(np.uint8)


def run_image(first, last):
    """The lossless WebP of the run of one level: red the first band, green the last band, blue 0."""
    return lossless(np.stack([first, last], -1))[1]


def snow_tile(below, own):
    """The bodies of the `snow` and the `snow-levels` tile from the runs (levels, 2, 256, 256) of its four per-year
    children {(dx, dy): runs or None}, or None when no child exists or no pixel has data. A missing child takes its
    part of `own`, the runs of the per-year tile itself, each pixel twice in each direction; without `own`, it has no
    data."""
    if all(child is None for child in below.values()):
        return None
    half = TILE // 2
    mosaic = np.full((len(LEVELS), 2, 2 * TILE, 2 * TILE), NO_DATA, np.uint8)
    for (dx, dy), child in below.items():
        if child is None and own is not None:
            child = own[:, :, dy * half:(dy + 1) * half, dx * half:(dx + 1) * half].repeat(2, 2).repeat(2, 3)
        if child is not None:
            mosaic[:, :, dy * TILE:(dy + 1) * TILE, dx * TILE:(dx + 1) * TILE] = child
    if (mosaic[HALF, 0] == NO_DATA).all():
        return None
    images = [run_image(first, last) for first, last in mosaic]
    return images[HALF], b"".join(images[:HALF] + images[HALF + 1:])


def worst(below):
    """A `snow-seasons` tile (seasons, 2, 64, 64) from the four tiles one zoom deeper {(dx, dy): planes or None}: in
    each season, a pixel takes the worst case of its 2 × 2 pixels. 254 when one of them is 254, else the earliest
    onset and the latest melt-out of those that are dated, else 253 when one of them is 253, else 255. A quarter of
    None stays 255."""
    seasons = next(child.shape[0] for child in below.values() if child is not None)
    half = SEASON_TILE // 2
    out = np.full((seasons, 2, SEASON_TILE, SEASON_TILE), NO_DATA, np.uint8)
    for (dx, dy), child in below.items():
        if child is None:
            continue
        # The 2 × 2 pixels of each pixel of the quarter on the last axis.
        four = child.reshape(seasons, 2, half, 2, half, 2).transpose(1, 0, 2, 4, 3, 5)
        onset, meltout = four.reshape(2, seasons, half, half, 4)
        dated = onset <= LAST_STEP
        full = (onset == FULL).any(-1)
        undated = np.where(full, FULL, np.where((onset == NO_SNOW).any(-1), NO_SNOW, NO_DATA))
        cases = np.stack([np.where(dated, onset, NO_DATA).min(-1), np.where(dated, meltout, 0).max(-1)], 1)
        quarter = np.where((dated.any(-1) & ~full)[:, None], cases, undated[:, None])
        out[:, :, dy * half:(dy + 1) * half, dx * half:(dx + 1) * half] = quarter
    return out


def season_tiles(planes, below):
    """The 16 `snow-seasons` tiles {(i, j): planes (seasons, 2, 64, 64)} of zoom z + 2 over a per-year tile of zoom z
    with `planes`, (i, j) the column and the row. The four tiles one zoom deeper of tile (i, j) are in child
    (i ÷ 2, j ÷ 2), whose 16 tiles `below` gives {(dx, dy): {(i, j): planes or None} or None}. Tile (i, j) is the
    worst case of those four; a quarter whose tile is None takes its part of `planes`."""
    size, half = SEASON_TILE, SEASON_TILE // 2
    out = {}
    for j in range(4):
        for i in range(4):
            part = planes[:, :, j * size:(j + 1) * size, i * size:(i + 1) * size]
            child = below.get((i // 2, j // 2)) or {}
            deeper = {(dx, dy): child.get((2 * (i % 2) + dx, 2 * (j % 2) + dy)) for dy in (0, 1) for dx in (0, 1)}
            if all(tile is None for tile in deeper.values()):
                out[i, j] = part.copy()
                continue
            tile = worst(deeper)
            for (dx, dy), deep in deeper.items():
                if deep is None:
                    rows, cols = slice(dy * half, (dy + 1) * half), slice(dx * half, (dx + 1) * half)
                    tile[:, :, rows, cols] = part[:, :, rows, cols]
            out[i, j] = tile
    return out


def make(z, x, y, planes, below, bounds, write):
    """Write the `snow` and `snow-levels` tile z/x/y and the `snow-seasons` tiles of zoom z + 2 over the per-year
    tile z/x/y with `planes`, with `write(archive, tile id, body)`. `below` gives the runs and the 16 `snow-seasons`
    tiles of each child {(dx, dy): (runs, tiles) or None}. Return the runs of the tile and its 16 tiles, None where a
    tile is not written: it does not meet the bounds or has no data."""
    from pmtiles.tile import zxy_to_tileid

    own = runs(planes)
    # A per-year tile of zoom 9 to 12 that the join lacks reads its ancestor (`season_tile`).
    made = snow_tile({at: child and child[0] for at, child in below.items()}, own if z >= MODIS_ZOOM else None)
    if made is not None:
        for archive, body in zip(("snow", "snow-levels"), made):
            write(archive, zxy_to_tileid(z, x, y), body)
    tiles = season_tiles(planes, {at: child[1] for at, child in below.items() if child})
    for (i, j), tile in tiles.items():
        at = (z + 2, 4 * x + i, 4 * y + j)
        if area(bounds, *at) is None or (tile == NO_DATA).all():
            tiles[i, j] = None
        else:
            write("snow-seasons", zxy_to_tileid(*at), gzipped(tile))
    return own, tiles


def canopy_cover(grid, paths):
    """Mean tree canopy cover in percent of each pixel of `grid`, in the year 2000, from the Hansen GFC
    `treecover2000` rasters `paths`."""
    import rasterio
    from rasterio.warp import Resampling, reproject

    cover = np.zeros(grid.shape, np.float32)
    for path in paths:
        with rasterio.open(path) as src:
            reproject(rasterio.band(src, 1), cover, dst_transform=grid.transform, dst_crs=grid.crs,
                      resampling=Resampling.average, init_dest_nodata=False)
    return cover


def area(bounds, z, x, y):
    """The part of the bounds in tile z/x/y, or None."""
    west, south, east, north = bounds
    w, s, e, n_ = geo.tile_bounds(z, x, y)
    if w >= east or e <= west or s >= north or n_ <= south:
        return None
    return max(w, west), max(s, south), min(e, east), min(n_, north)


def gzipped(body):
    # Level 9 takes much more time than level 6 for tiles that are barely smaller.
    return gzip.compress(body.tobytes(), compresslevel=6, mtime=0)


def write_archive(path, tiles, bounds, metadata, webp=False):
    """Write the PMTiles archive `path` of `tiles` {tile id: body}, in tile order, whose bodies are in memory or are
    (file, offset, length). The bodies are gzip of tile type `unknown`, or with `webp` WebP images without
    compression."""
    from pmtiles.tile import Compression, TileType, tileid_to_zxy
    from pmtiles.writer import Writer

    west, south, east, north = bounds
    e7 = lambda value: round(value * 1e7)
    top = max((tileid_to_zxy(tile_id)[0] for tile_id in tiles), default=0)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".snow-", delete=False) as stream:
        try:
            writer = Writer(stream)
            for tile_id in sorted(tiles):
                body = tiles[tile_id]
                if isinstance(body, tuple):
                    spill, offset, length = body
                    spill.seek(offset)
                    body = spill.read(length)
                writer.write_tile(tile_id, body)
            writer.finalize({
                "tile_type": TileType.WEBP if webp else TileType.UNKNOWN,
                "tile_compression": Compression.NONE if webp else Compression.GZIP,
                "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                "center_zoom": top, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
            }, metadata)
            stream.flush()
            os.replace(stream.name, path)
        except BaseException:
            os.unlink(stream.name)
            raise


def http_json(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)
        except (urllib.error.URLError, TimeoutError):
            if attempt == 4:
                raise
            time.sleep(5 * (attempt + 1))


def modis_grid(bounds):
    """The MODIS sinusoidal pixels around the bounds, with one pixel of margin."""
    from affine import Affine
    from rasterio.warp import transform_bounds

    left, bottom, right, top = transform_bounds("EPSG:4326", SINUSOIDAL, *bounds, densify_pts=100)
    size = MODIS_TILE_M / MODIS_PIXELS
    c0, c1 = math.floor((left - MODIS_X0) / size) - 1, math.ceil((right - MODIS_X0) / size) + 1
    r0, r1 = math.floor((MODIS_Y0 - top) / size) - 1, math.ceil((MODIS_Y0 - bottom) / size) + 1
    return Grid(SINUSOIDAL, Affine(size, 0, MODIS_X0 + c0 * size, 0, -size, MODIS_Y0 - r0 * size), (r1 - r0, c1 - c0))


def modis_files(paths, grid):
    """{date: [(platform, path)]} of the MODIS files `paths` whose tile meets `grid`: the Planetary Computer COG of a
    platform, day and tile, else its NSIDC granule."""
    size = grid.transform.a
    first_col, first_row = round((grid.transform.c - MODIS_X0) / size), round((MODIS_Y0 - grid.transform.f) / size)
    cols = range(first_col // MODIS_PIXELS, (first_col + grid.shape[1] - 1) // MODIS_PIXELS + 1)
    rows = range(first_row // MODIS_PIXELS, (first_row + grid.shape[0] - 1) // MODIS_PIXELS + 1)
    chosen = {}
    for path in paths:
        found = MODIS_FILE.fullmatch(Path(path).name)
        if found is None:
            raise ValueError(f"{path} is not a MOD10A1 or MYD10A1 file")
        platform, year, day, h, v, kind = found.groups()
        if int(h) in cols and int(v) in rows:
            key = (dt.date(int(year), 1, 1) + dt.timedelta(int(day) - 1), platform, h, v)
            if key not in chosen or kind.endswith(".tif"):
                chosen[key] = path
    days = {}
    for (day, platform, _, _), path in sorted(chosen.items()):
        days.setdefault(day, []).append((platform, path))
    return days


def modis_ndsi(path, grid):
    """The NDSI band of MODIS file `path` on `grid`, as (rows, cols, values), or None where it misses the grid. A
    `.hdf` file is an NSIDC granule, whose tile number places it on the sinusoidal grid."""
    size = grid.transform.a
    if str(path).endswith(".hdf"):
        h, v = MODIS_FILE.fullmatch(Path(path).name).group(4, 5)
        west, north = MODIS_X0 + int(h) * MODIS_TILE_M, MODIS_Y0 - int(v) * MODIS_TILE_M
        shape = (MODIS_PIXELS, MODIS_PIXELS)
    else:
        import rasterio

        with rasterio.open(path) as src:
            if abs(src.transform.a - size) > 1e-3:
                raise ValueError(f"{path} is not on the MODIS sinusoidal grid")
            west, north, shape = src.transform.c, src.transform.f, (src.height, src.width)
    col, row = (west - grid.transform.c) / size, (grid.transform.f - north) / size
    if max(abs(col - round(col)), abs(row - round(row))) * size > 1:
        raise ValueError(f"{path} is not on the MODIS sinusoidal grid")
    col, row = round(col), round(row)
    top, left = max(row, 0), max(col, 0)
    bottom, right = min(row + shape[0], grid.shape[0]), min(col + shape[1], grid.shape[1])
    if top >= bottom or left >= right:
        return None
    rows, cols = slice(top - row, bottom - row), slice(left - col, right - col)
    if str(path).endswith(".hdf"):
        from pyhdf.SD import SD, SDC

        values = SD(str(path), SDC.READ).select("NDSI_Snow_Cover")[rows, cols]
    else:
        import rasterio
        from rasterio.windows import Window

        with rasterio.open(path) as src:
            values = src.read(1, window=Window(cols.start, rows.start, cols.stop - cols.start, rows.stop - rows.start))
    return slice(top, bottom), slice(left, right), values


def modis_day(files, grid):
    """Clear and snow masks of one day from its files [(platform, path)]: Terra first, Aqua where Terra is not
    clear."""
    ndsi = {"MOD": np.full(grid.shape, NO_DATA, np.uint8), "MYD": np.full(grid.shape, NO_DATA, np.uint8)}
    for platform, path in files:
        if (placed := modis_ndsi(path, grid)) is not None:
            rows, cols, values = placed
            ndsi[platform][rows, cols] = values
    # 0–100 is NDSI × 100 and 254 a saturated detector; the other values are cloud, night, water or fill.
    clear = lambda value: (value <= 100) | (value == 254)
    value = np.where(clear(ndsi["MOD"]), ndsi["MOD"], ndsi["MYD"])
    return clear(value), clear(value) & (value >= MODIS_SNOW_NDSI)


def bounded_map(pool, function, items, ahead=64):
    """`pool.map(function, items)` with at most `ahead` items submitted and not yet read: the readers of MODIS days
    outrun `SnowSeason.observe`, and each day read holds its planes until it is observed."""
    pending = deque()
    for item in items:
        pending.append(pool.submit(function, item))
        if len(pending) == ahead:
            yield pending.popleft().result()
    while pending:
        yield pending.popleft().result()


def modis_season(days, grid, season, dense, pool):
    """The planes (1, 2, rows, cols) of `season` on the MODIS `grid` from the daily files {date: [(platform, path)]};
    `pool` reads the days. A pixel of the mask `dense` is no data."""
    first, count = dt.date(season, 9, 1), season_days(season)
    state = SnowSeason(count, grid.shape)
    dates = [day for day in sorted(days) if 0 <= (day - first).days < count]
    read = functools.partial(modis_day, grid=grid)
    for day, (clear, snow) in zip(dates, bounded_map(pool, read, [days[day] for day in dates])):
        state.observe((day - first).days, clear, snow)
    planes = state.planes()
    planes[:, :, dense] = NO_DATA
    return planes


def copernicus_files(bounds):
    """{season: {layer: [GDAL path]}} of the Snow Phenology S2 products that touch the bounds, on CDSE S3, each list
    in the order of the product names."""
    west, south, east, north = bounds
    footprint = f"POLYGON(({west} {south},{east} {south},{east} {north},{west} {north},{west} {south}))"
    # Without an order, the pages of a long listing can skip products.
    query = urllib.parse.urlencode({"$filter": f"contains(Name,'{COPERNICUS_PRODUCT}') and "
                                               f"OData.CSC.Intersects(area=geography'SRID=4326;{footprint}')",
                                    "$orderby": "Name", "$top": 1000})
    page, files = http_json(f"{ODATA}?{query}"), {}
    while True:
        for product in page["value"]:
            season = int(re.search(r"_(\d{4})0901P1Y_", product["Name"])[1])
            for layer in ("SCO", "SCM", "SCD"):
                files.setdefault(season, {}).setdefault(layer, []).append(f"/vsis3{product['S3Path']}/{product['Name']}_{layer}.tif")
        if "@odata.nextLink" not in page:
            return files
        page = http_json(page["@odata.nextLink"])


def cdse_credentials():
    """Point GDAL at CDSE S3 with the keys in CDSE_KEYS."""
    keys = dict(line.split("=", 1) for line in CDSE_KEYS.read_text().splitlines() if "=" in line and not line.startswith("#"))
    keys = {key.strip().removeprefix("export "): value.strip().strip("\"'") for key, value in keys.items()}
    # The 8 programs of a prepare open more connections than the 4 that CDSE allows a user: GDAL retries the HTTP
    # status of that refusal.
    os.environ.update(AWS_ACCESS_KEY_ID=keys["CDSE_S3_ACCESS_KEY"], AWS_SECRET_ACCESS_KEY=keys["CDSE_S3_SECRET_KEY"],
                      AWS_S3_ENDPOINT="eodata.dataspace.copernicus.eu", AWS_VIRTUAL_HOSTING="FALSE", AWS_HTTPS="YES",
                      AWS_REGION="default", GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_MAX_RETRY="8",
                      GDAL_HTTP_RETRY_DELAY="3")


@functools.cache
def laea_footprint(path):
    """The bounds of raster `path` in EPSG:3035. Each chunk tests every file, and `transform_bounds` holds the GIL
    while it sets up a new transform, so each file pays for it once in each process."""
    import rasterio
    from rasterio.warp import transform_bounds

    with rasterio.open(path) as src:
        return transform_bounds(src.crs, "EPSG:3035", *src.bounds, densify_pts=100)


def copernicus_season(files, bounds, days):
    """The planes (1, 2, rows, cols) of a season of `days` days on a 20 m LAEA grid around the bounds, and the grid,
    from its Snow Phenology S2 rasters {layer: [path]}. A later raster wins where two overlap."""
    import rasterio
    from affine import Affine
    from rasterio.warp import Resampling, reproject, transform_bounds
    from rasterio.windows import Window, from_bounds, intersect

    left, bottom, right, top = transform_bounds("EPSG:4326", "EPSG:3035", *bounds, densify_pts=100)
    left, top = math.floor(left / 20) * 20 - 20, math.ceil(top / 20) * 20 + 20
    grid = Grid("EPSG:3035", Affine(20, 0, left, 0, -20, top),
                (math.ceil((top - bottom) / 20) + 1, math.ceil((right - left) / 20) + 1))
    extent = (left, top - 20 * grid.shape[0], left + 20 * grid.shape[1], top)
    layers = {}
    for name in ("SCO", "SCM", "SCD"):
        # 65535 is no data and 420 inland water.
        layer = np.full(grid.shape, 65535, np.uint16)
        for path in files.get(name, []):
            west, south, east, north = laea_footprint(path)
            if west >= extent[2] or east <= extent[0] or south >= extent[3] or north <= extent[1]:
                continue
            with rasterio.open(path) as src:
                window = from_bounds(*transform_bounds(grid.crs, src.crs, *extent, densify_pts=100), src.transform)
                window, raster = window.round_offsets().round_lengths(), Window(0, 0, src.width, src.height)
                # The catalogue footprint of a product can reach past its raster.
                if not intersect(window, raster):
                    continue
                window = window.intersection(raster)
                reproject(src.read(1, window=window), layer, src_transform=src.window_transform(window), src_crs=src.crs,
                          dst_transform=grid.transform, dst_crs=grid.crs, resampling=Resampling.nearest,
                          src_nodata=65535, dst_nodata=65535, init_dest_nodata=False)
        layers[name] = layer.astype(np.int32)
    onset, meltout, duration = layers["SCO"], layers["SCM"], layers["SCD"]
    data = (duration <= 366) & ((duration == 0) | (onset <= 366) & (meltout <= 366))
    length = np.where(duration == 0, 0, meltout - onset + 1)
    return encode(onset, length, days, data)[None], grid


# What the chunks of the season that a block makes read: the workers fork from the process, so they read the MODIS
# planes without a copy.
Making = namedtuple("Making", "hr modis days bounds season slots")
_making = None


def merged(hr, z, x, y):
    """Tile z/x/y of the archive from its HR-WSI tile: MODIS where HR-WSI is 255."""
    if _making.modis is None:
        return hr
    return np.where(hr == NO_DATA, sample(*_making.modis, z, x, y), hr)


def chunk(tile):
    """The HR-WSI tile of chunk tile (x, y), or None without HR-WSI data in it or below it, and the gzip tiles
    {tile id: body} of the chunk and below that the season has."""
    from pmtiles.tile import zxy_to_tileid

    x, y = tile
    m, tiles = _making, {}
    part = area(m.bounds, CHUNK_ZOOM, x, y) if m.bounds else geo.tile_bounds(CHUNK_ZOOM, x, y)
    if not m.hr or part is None:
        return None, tiles
    with cdse_slot(m.slots):
        planes = copernicus_season(m.hr, part, m.days)
    if (planes[0] == NO_DATA).all():
        return None, tiles

    def down(z, x, y):
        if m.bounds and area(m.bounds, z, x, y) is None:
            return None
        if z == HR_ZOOM:
            body = sample(*planes, z, x, y)
            if (body == NO_DATA).all():
                return None
        else:
            below = {at: down(*child) for at, child in children(z, x, y)}
            if all(body is None for body in below.values()):
                return None
            body = parent(below, 1, smoothed=True)
        tiles[zxy_to_tileid(z, x, y)] = gzipped(merged(body, z, x, y))
        return body

    return down(CHUNK_ZOOM, x, y), tiles


def make_season(block, making, out):
    """Write the file of the season of `making` of `block` (x, y) to `out`; return its SHA-256."""
    from pmtiles.tile import zxy_to_tileid

    global _making
    bx, by = block
    _making, tiles, hr = making, {}, {}
    scale = 1 << (CHUNK_ZOOM - BLOCK_ZOOM)
    chunks = [(x, y) for x in range(bx * scale, (bx + 1) * scale) for y in range(by * scale, (by + 1) * scale)]
    try:
        with step_pool.pool(WORKERS) as pool:
            for tile, (body, made) in zip(chunks, pool.map(chunk, chunks)):
                hr[tile] = body
                tiles.update(made)

        def up(z, x, y):
            """The HR-WSI and the MODIS tile of z/x/y, each None where the source has none, and whether the season
            has the tile."""
            if z == CHUNK_ZOOM:
                return hr[(x, y)], None, hr[(x, y)] is not None
            below = {at: up(*child) for at, child in children(z, x, y)}
            held = any(has for _, _, has in below.values())
            hr_tile = parent({at: tile for at, (tile, _, _) in below.items()}, 1, smoothed=True) if held else None
            if z > MODIS_ZOOM:
                if held:
                    tiles[zxy_to_tileid(z, x, y)] = gzipped(merged(hr_tile, z, x, y))
                return hr_tile, None, held
            modis = None
            if making.modis is not None and (not making.bounds or area(making.bounds, z, x, y)):
                if z == MODIS_ZOOM:
                    modis = sample(*making.modis, z, x, y)
                elif any(m is not None for _, m, _ in below.values()):
                    modis = parent({at: m for at, (_, m, _) in below.items()}, 1)
                if modis is not None and (modis == NO_DATA).all():
                    modis = None
            body = hr_tile if modis is None else modis if hr_tile is None else np.where(hr_tile == NO_DATA, modis, hr_tile)
            has = held or (body is not None and (body != NO_DATA).any())
            if has:
                tiles[zxy_to_tileid(z, x, y)] = gzipped(body)
            return hr_tile, modis, has

        up(BLOCK_ZOOM, bx, by)
    finally:
        _making = None
    path = out / f"{making.season}/{bx}/{by}.pmtiles"
    if tiles:
        write_archive(path, tiles, geo.tile_bounds(BLOCK_ZOOM, bx, by), {"season": making.season})
    else:
        # PMTiles has no archive without a tile.
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"")
    return hashlib.sha256(path.read_bytes()).hexdigest()


@contextlib.contextmanager
def cdse_slot(directory):
    """Hold one of the CDSE_CONNECTIONS lock files in `directory`, waiting for a free one; no limit without one."""
    if directory is None:
        yield
        return
    import fcntl

    directory.mkdir(parents=True, exist_ok=True)
    while True:
        for k in range(CDSE_CONNECTIONS):
            with open(directory / f"{k}.lock", "w") as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    continue
                yield
                return
        time.sleep(0.2)


def hr_wsi_files(block, seasons):
    """The Snow Phenology products of `block` as `copernicus_files` gives them. A listing with products of a season and
    none of another of `seasons` fails, as a short page of the catalogue would make that season MODIS only for good;
    and so does a listing whose seasons of `seasons` have other tiles. An empty listing is asked once more, as a block
    outside HR-WSI or at sea has none."""
    edges = geo.tile_bounds(BLOCK_ZOOM, *block)
    files = copernicus_files(edges)
    if not files:
        time.sleep(10)
        files = copernicus_files(edges)
    name = f"block {block[0]}-{block[1]}: the HR-WSI catalogue"
    if files and (lacking := [season for season in seasons if season not in files]):
        raise SystemExit(f"{name} lists products of seasons {sorted(files)} but none of {lacking}; try again later")
    # `CLMS_WSI_SP_020m_<tile>_<season>0901P1Y_…`: a season of a tile has one product.
    tiles = {season: {Path(path).name.split("_")[4] for path in files[season]["SCO"]} for season in seasons if season in files}
    if differ := [season for season in tiles if tiles[season] != tiles[min(tiles)]]:
        raise SystemExit(f"{name}'s tile set differs between seasons {min(tiles)} and {differ[0]}")
    return files


def prepare(block, seasons, modis, canopy, out, bounds=None, slots=None):
    """Write `<season>/<x>/<y>.pmtiles` of `block` (x, y) of `snow` into `out` for each of `seasons`, each followed by
    its mark `<x>-<y>.<season>.sha256`, the SHA-256 of the file. `modis` names the MODIS files and `canopy` the Hansen
    GFC rasters; without `modis`, the files have HR-WSI only. `bounds` keeps only the tiles that meet them. `slots` is
    the directory of the CDSE connections that the programs of a prepare share."""
    bx, by = block
    edges = geo.tile_bounds(BLOCK_ZOOM, bx, by)
    cdse_credentials()
    files = hr_wsi_files(block, seasons)
    grid = modis_grid(edges) if modis is not None else None
    days = modis_files(modis, grid) if modis is not None else {}
    dense = canopy_cover(grid, canopy) > DENSE_CANOPY_PERCENT if modis is not None else None
    for season in sorted(seasons):
        mark = out / f"{bx}-{by}.{season}.sha256"
        if mark.exists() and (out / f"{season}/{bx}/{by}.pmtiles").exists():
            continue
        planes = None
        if modis is not None:
            # rasterio holds the GIL for much of a file open, so the reads need processes.
            with step_pool.pool(WORKERS) as pool:
                planes = modis_season(days, grid, season, dense, pool), grid
        making = Making(files.get(season, {}), planes, season_days(season), bounds, season, slots)
        mark.write_text(make_season(block, making, out) + "\n")
        print(f"block {bx}-{by}: season {season}", file=sys.stderr, flush=True)


def tile_index(get_bytes):
    """{(z, x, y): (offset, length)} of the tiles of a PMTiles archive."""
    from pmtiles.tile import deserialize_directory, deserialize_header, tileid_to_zxy

    header = deserialize_header(get_bytes(0, 127))
    index, todo = {}, [(header["root_offset"], header["root_length"])]
    while todo:
        offset, length = todo.pop()
        for entry in deserialize_directory(get_bytes(offset, length)):
            if entry.run_length == 0:
                todo.append((header["leaf_directory_offset"] + entry.offset, entry.length))
            for i in range(entry.run_length):
                index[tileid_to_zxy(entry.tile_id + i)] = (header["tile_data_offset"] + entry.offset, entry.length)
    return index


def season_tile(get_bytes, index, z, x, y):
    """Tile z/x/y (1, 2, 256, 256) of a season file: its own, else for zoom 9 to 12 the part of the nearest ancestor
    of zoom 8 or more, each pixel repeated, else no data."""
    for k in range(z - MODIS_ZOOM + 1 if z > MODIS_ZOOM else 1):
        if (at := index.get((z - k, x >> k, y >> k))) is not None:
            body = np.frombuffer(gzip.decompress(get_bytes(*at)), np.uint8).reshape(1, 2, TILE, TILE)
            scale, size = 1 << k, TILE >> k
            row, col = (y % scale) * size, (x % scale) * size
            return body[:, :, row:row + size, col:col + size].repeat(scale, 2).repeat(scale, 3)
    return np.full((1, 2, TILE, TILE), NO_DATA, np.uint8)


def join_block(job):
    """The tiles of the three archives over the per-year tiles of block (x, y) that meet `bounds`, from its season
    files in season order, written to the file `spill`: {archive: {tile id: (offset, length)}}; the planes and the
    runs of the per-year zoom 6 tile, or None; and whether the block has a per-year tile of zoom 9 or more.

    The walk is depth first, so it holds only the tiles of one path from zoom 6 to 12 and the runs and the
    `snow-seasons` tiles of their children."""
    from pmtiles.reader import MmapSource

    (bx, by), paths, bounds, spill = job
    opened = [open(path, "rb") for path in paths]
    try:
        sources = []
        for stream in opened:
            # The file of a season without data in the block is empty.
            get = MmapSource(stream) if os.fstat(stream.fileno()).st_size else None
            sources.append((get, tile_index(get) if get else {}))
        # A season file holds each ancestor of its tiles down to zoom 6, so the walk from zoom 6 reaches each tile.
        present = {tile for _, index in sources for tile in index if area(bounds, *tile)}
        made = {archive: {} for archive in ARCHIVES}
        with open(spill, "wb") as stream:
            def write(archive, tile_id, body):
                made[archive][tile_id] = (stream.tell(), len(body))
                stream.write(body)

            def visit(z, x, y):
                below = {at: visit(*child) if child in present else None for at, child in children(z, x, y)
                         if z < HR_ZOOM}
                planes = np.concatenate([season_tile(get, index, z, x, y) for get, index in sources])
                own, tiles = make(z, x, y, planes, below, bounds, write)
                return (planes, own) if z == BLOCK_ZOOM else (own, tiles)

            top = visit(BLOCK_ZOOM, bx, by) if (BLOCK_ZOOM, bx, by) in present else None
        return made, top, any(z > MODIS_ZOOM for z, _, _ in present)
    finally:
        for stream in opened:
            stream.close()


def join(files, bounds, seasons, output, credit):
    """Write `snow.pmtiles`, `snow-levels.pmtiles` and `snow-seasons.pmtiles` of `bounds` into the folder `output`,
    with the attribution `credit`, from the files of `snow` ({path: file}) of `seasons`; return {archive: (tiles,
    bytes)}. The per-year tiles of zoom 0 to 5 blend the zoom 6 tiles of the blocks."""
    from pmtiles.tile import zxy_to_tileid

    blocks = {}
    for name, path in files.items():
        season, x, y = map(int, re.fullmatch(r"(\d{4})/(\d+)/(\d+)\.pmtiles", name).groups())
        blocks.setdefault((x, y), {})[season] = path
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output, prefix=".snow-") as spills:
        jobs = [(block, [blocks[block][season] for season in seasons], bounds, f"{spills}/{block[0]}-{block[1]}")
                for block in sorted(blocks)]
        tiles, tops, opened, hr = {archive: {} for archive in ARCHIVES}, {}, [], False
        try:
            with step_pool.pool() as pool:
                for job, (made, top, deep) in zip(jobs, pool.map(join_block, jobs)):
                    opened.append(stream := open(job[3], "rb"))
                    for archive, placed in made.items():
                        tiles[archive].update({tile_id: (stream, *at) for tile_id, at in placed.items()})
                    tops[job[0]], hr = top, hr or deep

            def stored(z, x, y):
                if (at := tiles["snow-seasons"].get(zxy_to_tileid(z, x, y))) is None:
                    return None
                stream, offset, length = at
                stream.seek(offset)
                body = gzip.decompress(stream.read(length))
                return np.frombuffer(body, np.uint8).reshape(len(seasons), 2, SEASON_TILE, SEASON_TILE)

            def write(archive, tile_id, body):
                tiles[archive][tile_id] = body

            def above(z, x, y):
                """The planes, the runs and the 16 `snow-seasons` tiles of the per-year tile z/x/y, or None."""
                if area(bounds, z, x, y) is None:
                    return None
                if z == BLOCK_ZOOM:
                    if (top := tops.get((x, y))) is None:
                        return None
                    return *top, {(i, j): stored(z + 2, 4 * x + i, 4 * y + j) for j in range(4) for i in range(4)}
                below = {at: above(*child) for at, child in children(z, x, y)}
                if all(child is None for child in below.values()):
                    return None
                planes = parent({at: child and child[0] for at, child in below.items()}, len(seasons))
                below = {at: child and child[1:] for at, child in below.items()}
                return planes, *make(z, x, y, planes, below, bounds, write)

            above(0, 0, 0)
            if not all(tiles.values()):
                raise ValueError("The snow files of the region have no tile")
            source = "copernicus-hr-wsi" if hr else "nasa-modis"
            metadata = {"snow_format": FORMAT, "first_season": seasons[0], "seasons": len(seasons), "source": source,
                        "resolution_m": RESOLUTION_M[source], "attribution": credit}
            for archive, webp in ARCHIVES.items():
                extra = {} if webp else {"step_days": 2}
                write_archive(output / f"{archive}.pmtiles", tiles[archive], bounds, {**metadata, **extra}, webp)
        finally:
            for stream in opened:
                stream.close()
    size = lambda body: body[2] if isinstance(body, tuple) else len(body)
    return {archive: (len(held), sum(map(size, held.values()))) for archive, held in tiles.items()}


def step():
    """The `obc data` step `planner/snow`: the three archives from the files of `snow` in its snapshot."""
    request = step_request.read()
    options = request["options"]
    first, last = options["seasons"]
    made = join(step_request.files(request, "snow"), options["bounds"], list(range(first, last + 1)),
                Path(request["output"]), options["attribution"].format(year=options["year"]))
    step_request.metrics(request, {archive: {"tiles": count, "bytes": size} for archive, (count, size) in made.items()})


def box(bounds, output):
    """Bake the HR-WSI seasons of `bounds` into the three archives in the folder `output`, without MODIS."""
    # The step gets its credit in its options, so only this command line reads the registry.
    from . import data_registry

    cdse_credentials()
    listed = copernicus_files(bounds)
    seasons = list(range(min(listed), max(listed) + 1))
    west, south, east, north = bounds
    (x0, y0), (x1, y1) = geo.mercator(west, north, BLOCK_ZOOM), geo.mercator(east, south, BLOCK_ZOOM)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output, prefix=".snow-box-") as out:
        files = {}
        for x in range(int(x0), int(x1) + 1):
            for y in range(int(y0), int(y1) + 1):
                prepare((x, y), seasons, None, None, Path(out), bounds)
                files.update({f"{season}/{x}/{y}.pmtiles": Path(out, f"{season}/{x}/{y}.pmtiles") for season in seasons})
        join(files, bounds, seasons, output, data_registry.attribution("hr-wsi", year=dt.date.today().year))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("prepare", help="the files of one block of `snow`")
    command.add_argument("--block", type=lambda text: tuple(map(int, text.split("-"))), required=True, help="X-Y")
    command.add_argument("--seasons", type=lambda text: [int(season) for season in text.split(",")], required=True)
    command.add_argument("--modis", type=Path, required=True, help="a file that names the MODIS files, one a line")
    command.add_argument("--canopy", type=lambda text: text.split(","), required=True, help="the Hansen GFC rasters")
    command.add_argument("--slots", type=Path, help="the lock files of the CDSE connections of the prepare")
    command.add_argument("--out", type=Path, required=True)
    command = commands.add_parser("box", help="bake the HR-WSI seasons of a box, without MODIS")
    command.add_argument("--bounds", type=geo.bounds, required=True, help="west,south,east,north")
    command.add_argument("--output", type=Path, required=True, help="the folder of the three archives")
    args = parser.parse_args()
    if args.command == "box":
        box(args.bounds, args.output)
    else:
        prepare(args.block, args.seasons, args.modis.read_text().splitlines(), args.canopy, args.out, slots=args.slots)


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
