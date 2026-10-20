"""The tiles of the prepared source `forest-reference/<key>`, as specs/obc-data.md defines them: per
cell of a zoom 12 tile at 256 px, the canopy height, the cover, the stems per hectare and the conifer
share.

    python -m tools.forest_reference fetch ch --tile 12-2143-1443 --out DIR
    python -m tools.forest_reference index-tiles ch --fetched DAY --digests FILE --out FILE

`fetch` reads the source rasters through HTTP range requests and keeps no raw raster on disk.
"""

import argparse
import hashlib
import json
import math
import os
import signal
import sys
import threading
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from numba import njit

ZOOM, SIZE = 12, 256
WORLD = 2 * math.pi * 6378137
CELL = WORLD / (SIZE << ZOOM)
# The cell rows of one window: a band of 0.84 km, so the rasters of a window stay below 1 GB.
BAND = 32
# A pixel this high is canopy: the minimum top height of the Swiss forest definition.
CANOPY_M = 3.0
# The lowest tree top that counts as a stem.
TOP_M = 5.0
# The tallest canopy that a cell states: a higher pixel is an error, such as a cable over a valley.
TALLEST_DM = 800
NODATA = 65535
# The overview of the vegetation height model that shows where an acquisition has data: 256 m.
COARSE = 8
BANDS = ("height_dm", "cover_pct", "stems_per_ha", "conifer_pct")

VHM = ("https://data.geo.admin.ch/api/stac/v0.9/collections/"
       "ch.bafu.landesforstinventar-vegetationshoehenmodell_lidar/items")
DLT = ("https://image.discomap.eea.europa.eu/arcgis/rest/services/GioLandPublic/"
       "HRL_DominanteLeafType_2018/ImageServer/exportImage")


@njit(nogil=True, cache=True)
def smoothed(heights):
    """The binomial 3 × 3 mean of `heights` (unknown as 0 m), which removes the maxima of single
    branches."""
    rows, cols = heights.shape
    level = np.zeros((rows, cols), np.float32)
    for i in range(rows):
        for j in range(cols):
            total, weight = 0.0, 0.0
            for di in range(-1, 2):
                for dj in range(-1, 2):
                    r, c = i + di, j + dj
                    if 0 <= r < rows and 0 <= c < cols:
                        w = (2 - abs(di)) * (2 - abs(dj))
                        value = heights[r, c]
                        total += w * (0.0 if math.isnan(value) else value)
                        weight += w
            level[i, j] = total / weight
    return level


@njit(nogil=True, cache=True)
def crown_radius(height, pixel_m):
    """The crown radius in pixels: 0.5 m + 8 % of the height, from 1 m to 4 m, as the crown of a
    Central European conifer grows."""
    return min(max(0.5 + 0.08 * height, 1.0), 4.0) / pixel_m


@njit(nogil=True, cache=True)
def root(parent, p):
    while parent[p] != p:
        parent[p] = parent[parent[p]]
        p = parent[p]
    return p


@njit(nogil=True, cache=True)
def crown_tops(level, pixel_m, top_m, offsets):
    """The tree tops of the smoothed canopy `level`. A peak is a pixel at or above `top_m` that no
    pixel within its crown radius exceeds. Peaks of one height within the crown radius of each
    other, directly or through other such peaks, are one tree, such as a flat crown: its top is the
    first of them in raster order. `offsets` are the pixel offsets nearest first: most pixels are
    not peaks, and a near pixel shows it."""
    rows, cols = level.shape
    parent = np.full(rows * cols, -1, np.int64)
    for i in range(rows):
        for j in range(cols):
            height = level[i, j]
            if height < top_m:
                continue
            radius = crown_radius(height, pixel_m)
            peak = True
            for di, dj in offsets:
                if di * di + dj * dj > radius * radius:
                    break
                r, c = i + di, j + dj
                if 0 <= r < rows and 0 <= c < cols and level[r, c] > height:
                    peak = False
                    break
            if peak:
                parent[i * cols + j] = i * cols + j
    for i in range(rows):
        for j in range(cols):
            p = i * cols + j
            if parent[p] < 0:
                continue
            radius = crown_radius(level[i, j], pixel_m)
            for di, dj in offsets:
                if di * di + dj * dj > radius * radius:
                    break
                r, c = i + di, j + dj
                earlier = di < 0 or di == 0 and dj < 0
                if earlier and 0 <= r < rows and 0 <= c < cols and parent[r * cols + c] >= 0 and level[r, c] == level[i, j]:
                    a, b = root(parent, p), root(parent, r * cols + c)
                    parent[max(a, b)] = min(a, b)
    tops = np.zeros((rows, cols), np.bool_)
    for p in range(rows * cols):
        if parent[p] >= 0 and root(parent, p) == p:
            tops[p // cols, p % cols] = True
    return tops


def tree_tops(heights, pixel_m):
    """The mask of the tree tops of the canopy height model `heights` (metres, NaN unknown). A
    flat crown that reaches more than `halo(pixel_m)` past the edge of a window can count once on
    each side."""
    reach = math.ceil(4.0 / pixel_m)
    offsets = np.array([(di, dj) for di in range(-reach, reach + 1) for dj in range(-reach, reach + 1) if di or dj])
    offsets = offsets[np.argsort((offsets ** 2).sum(axis=1), kind="stable")]
    return crown_tops(smoothed(np.asarray(heights, np.float32)), pixel_m, TOP_M, offsets)


def halo(pixel_m):
    """The pixels around a window that its tree tops depend on: the widest crown radius, and one
    pixel of the smoothing."""
    return math.ceil(4.0 / pixel_m) + 1


@njit(nogil=True, cache=True)
def lattice_cells(lattice_j, lattice_i, at_rows, at_cols, shape, band_rows, columns):
    """The cell of each pixel from the cell coordinates of the lattice points at every 64th row
    `at_rows` and column `at_cols` and the last, bilinear in between: -1 outside `band_rows` ×
    `columns` cells."""
    cells = np.full(shape, -1, np.int32)
    for r in range(shape[0]):
        k = min(r // 64, len(at_rows) - 2)
        t = (r - at_rows[k]) / (at_rows[k + 1] - at_rows[k])
        for c in range(shape[1]):
            m = min(c // 64, len(at_cols) - 2)
            u = (c - at_cols[m]) / (at_cols[m + 1] - at_cols[m])
            j = ((1 - t) * ((1 - u) * lattice_j[k, m] + u * lattice_j[k, m + 1])
                 + t * ((1 - u) * lattice_j[k + 1, m] + u * lattice_j[k + 1, m + 1]))
            i = ((1 - t) * ((1 - u) * lattice_i[k, m] + u * lattice_i[k, m + 1])
                 + t * ((1 - u) * lattice_i[k + 1, m] + u * lattice_i[k + 1, m + 1]))
            if 0 <= i < band_rows and 0 <= j < columns:
                cells[r, c] = int(i) * columns + int(j)
    return cells


def band_cells(x, y, ci, rows, cols, transform):
    """The cell of each pixel of the raster rows `rows` and columns `cols` (LV95 grid `transform`) in
    the band of tile x/y whose first cell row is `ci`, or -1. The exact projection runs on a lattice
    of 64 pixels; in between, the projection is affine to within millimetres."""
    from rasterio.warp import transform as project

    height, width = rows[1] - rows[0], cols[1] - cols[0]
    at_rows = np.unique(np.r_[np.arange(0, height, 64), height - 1])
    at_cols = np.unique(np.r_[np.arange(0, width, 64), width - 1])
    grid_rows, grid_cols = np.meshgrid(at_rows + rows[0] + 0.5, at_cols + cols[0] + 0.5, indexing="ij")
    east, north = transform * (grid_cols, grid_rows)
    mx, my = (np.reshape(values, east.shape) for values in project("EPSG:2056", "EPSG:3857", east.ravel(), north.ravel()))
    west, top = tile_origin(x, y)
    return lattice_cells((mx - west) / CELL, (top - my) / CELL - ci, at_rows, at_cols, (height, width), BAND, SIZE)


@njit(nogil=True, cache=True)
def accumulate(cells, heights, tops, count, canopy_m, tallest_dm):
    """The known pixels, the canopy pixels, the tree tops and the histogram of the canopy heights in
    decimetres of each of `count` cells."""
    pixels, crowns, stems = np.zeros(count, np.int64), np.zeros(count, np.int64), np.zeros(count, np.int64)
    histogram = np.zeros((count, tallest_dm + 1), np.int32)
    for r in range(cells.shape[0]):
        for c in range(cells.shape[1]):
            cell, height = cells[r, c], heights[r, c]
            if cell < 0 or math.isnan(height):
                continue
            pixels[cell] += 1
            stems[cell] += tops[r, c]
            if height >= canopy_m:
                crowns[cell] += 1
                histogram[cell, min(int(height * 10), tallest_dm)] += 1
    return pixels, crowns, stems, histogram


def cell_stats(cells, heights, tops, count, pixel_m=0.5):
    """The height, cover and stems bands of `count` cells from the pixels of their windows: `cells`
    gives the cell of each pixel, or -1. A cell without a known pixel stays `NODATA`."""
    pixels, crowns, stems, histogram = accumulate(cells, heights, tops, count, CANOPY_M, TALLEST_DM)
    # The 95th percentile of the canopy pixels of each cell, in decimetres.
    p95 = np.argmax(histogram.cumsum(axis=1) > np.floor(0.95 * (crowns - 1))[:, None], axis=1)
    out = np.full((3, count), NODATA, np.uint16)
    seen = pixels > 0
    hectares = pixels[seen] * pixel_m * pixel_m / 1e4
    out[0, seen] = np.where(crowns[seen] > 0, p95[seen], 0)
    out[1, seen] = np.rint(100 * crowns[seen] / pixels[seen])
    out[2, seen] = np.minimum(np.rint(stems[seen] / hectares), NODATA - 1)
    return out


def tile_origin(x, y):
    """The west and north edges of tile x/y in Web Mercator metres."""
    return x * SIZE * CELL - WORLD / 2, WORLD / 2 - y * SIZE * CELL


def http(url, tries=5):
    for attempt in range(tries):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "openbikecomputer"})
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.read()
        except OSError:
            if attempt == tries - 1:
                raise
            time.sleep(2 ** attempt)


def vhm_years():
    """The rasters of the vegetation height model, newest acquisition first."""
    items = json.loads(http(f"{VHM}?limit=100"))["features"]
    hrefs = [(item["properties"]["datetime"], asset["href"]) for item in items
             for asset in item["assets"].values() if asset["href"].endswith(".tif")]
    return [href for _, href in sorted(hrefs, reverse=True)]


def leaf_type(x, y):
    """The conifer share of the tree pixels of each cell, in %, from HRL Dominant Leaf Type 2018
    (1 broadleaved, 2 coniferous) at four sub-pixels per cell side; `NODATA` without a tree pixel."""
    import rasterio
    from rasterio.io import MemoryFile

    west, top = tile_origin(x, y)
    query = {"bbox": f"{west},{top - SIZE * CELL},{west + SIZE * CELL},{top}", "bboxSR": 3857, "imageSR": 3857,
             "size": f"{4 * SIZE},{4 * SIZE}", "format": "tiff", "pixelType": "U8",
             "interpolation": "RSP_NearestNeighbor", "f": "image"}
    with MemoryFile(http(f"{DLT}?{urllib.parse.urlencode(query)}")) as memory, memory.open() as image:
        if image.crs != rasterio.crs.CRS.from_epsg(3857) or image.shape != (4 * SIZE, 4 * SIZE):
            raise ValueError(f"the leaf type service gave {image.crs} {image.shape}")
        classes = image.read(1).reshape(SIZE, 4, SIZE, 4)
    broad, conifer = ((classes == value).sum(axis=(1, 3)) for value in (1, 2))
    trees = broad + conifer
    return np.where(trees > 0, np.rint(100 * conifer / np.maximum(trees, 1)), NODATA).astype(np.uint16)


GDAL = {"GDAL_CACHEMAX": 512, "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR", "GDAL_HTTP_MAX_RETRY": 5,
        "GDAL_HTTP_RETRY_DELAY": 2, "GDAL_NUM_THREADS": 4, "GDAL_HTTP_MULTIPLEX": "YES"}


def tile_ch(x, y):
    """The bands of tile x/y from the newest acquisition of the vegetation height model LiDAR NFI
    at each pixel, or None where it has no pixel. One thread reads the bands of the tile while the
    other reduces them."""
    import rasterio
    from rasterio.warp import transform_bounds

    def opened(href):
        """The raster, and where it has data: the valid pixels of its overview of 256 m."""
        with rasterio.Env(**GDAL):
            raster = rasterio.open(f"/vsicurl/{href}")
            factor = raster.overviews(1)[COARSE]
            with rasterio.open(f"/vsicurl/{href}", overview_level=COARSE) as overview:
                return raster, (overview.read(1) != overview.nodata, factor)

    def window(ci):
        bounds = (west, top - (ci + BAND) * CELL, west + SIZE * CELL, top - ci * CELL)
        left, bottom, right, upper = transform_bounds("EPSG:3857", "EPSG:2056", *bounds, densify_pts=32)
        (r0, r1), (c0, c1) = (sorted(values) for values in rasterio.transform.rowcol(
            grid.transform, [left, right], [upper, bottom], op=math.floor))
        return (max(r0 - margin, 0), min(r1 + 1 + margin, grid.height)), (max(c0 - margin, 0), min(c1 + 1 + margin, grid.width))

    def read(rows, cols):
        with rasterio.Env(**GDAL):
            return read_newest(rasters, coverage, rows, cols)

    out = np.full((4, SIZE, SIZE), NODATA, np.uint16)
    # Each open waits for the header of a remote file, so they wait together.
    with ThreadPoolExecutor(8) as pool:
        rasters, coverage = zip(*pool.map(opened, vhm_years()))
    try:
        grid = rasters[0]
        if any(raster.transform != grid.transform or raster.shape != grid.shape for raster in rasters):
            raise ValueError("the acquisitions of the vegetation height model have different grids")
        pixel_m, (west, top) = grid.res[0], tile_origin(x, y)
        margin = halo(pixel_m)
        windows = {ci: window(ci) for ci in range(0, SIZE, BAND)}
        with ThreadPoolExecutor(1) as reader:
            reads = {ci: reader.submit(read, rows, cols) for ci, (rows, cols) in windows.items()
                     if rows[0] < rows[1] and cols[0] < cols[1]}
            for ci, pending in reads.items():
                heights = pending.result()
                if heights is None:
                    continue
                rows, cols = windows[ci]
                cells = band_cells(x, y, ci, rows, cols, grid.transform)
                tops = tree_tops(heights, pixel_m)
                out[:3, ci:ci + BAND] = cell_stats(cells, heights, tops, BAND * SIZE, pixel_m).reshape(3, BAND, SIZE)
                del heights, cells, tops
    finally:
        for raster in rasters:
            raster.close()
    if (out[0] == NODATA).all():
        return None
    out[3] = np.where(out[0] != NODATA, leaf_type(x, y), NODATA)
    return out


def read_newest(rasters, coverage, rows, cols):
    """The heights of the raster rows `rows` and columns `cols` from the first of `rasters` that
    has each pixel, NaN where none has it, or None when none has a pixel. A raster is read only
    over the rows and columns that are still unknown and where `coverage`, its valid coarse pixels
    and their size in pixels, shows data."""
    from rasterio.windows import Window

    heights = np.full((rows[1] - rows[0], cols[1] - cols[0]), np.nan, np.float32)
    for raster, (valid, factor) in zip(rasters, coverage):
        unknown = np.isnan(heights)
        if not unknown.any():
            break
        at_rows, at_cols = np.flatnonzero(unknown.any(axis=1)), np.flatnonzero(unknown.any(axis=0))
        r0, r1, c0, c1 = rows[0] + at_rows[0], rows[0] + at_rows[-1] + 1, cols[0] + at_cols[0], cols[0] + at_cols[-1] + 1
        near = valid[r0 // factor:(r1 - 1) // factor + 1, c0 // factor:(c1 - 1) // factor + 1]
        if not near.any():
            continue
        has_rows, has_cols = np.flatnonzero(near.any(axis=1)), np.flatnonzero(near.any(axis=0))
        base_r, base_c = r0 // factor * factor, c0 // factor * factor
        r0, r1 = max(r0, base_r + has_rows[0] * factor), min(r1, base_r + (has_rows[-1] + 1) * factor)
        c0, c1 = max(c0, base_c + has_cols[0] * factor), min(c1, base_c + (has_cols[-1] + 1) * factor)
        values = read_window(raster, Window(c0, r0, c1 - c0, r1 - r0))
        values[values == raster.nodata] = np.nan
        part = heights[r0 - rows[0]:r1 - rows[0], c0 - cols[0]:c1 - cols[0]]
        np.copyto(part, values, where=np.isnan(part))
    return None if np.isnan(heights).all() else heights


def read_window(raster, window, tries=4):
    """The pixels of `window`. A cut range request gives a broken block, so a read tries again."""
    import rasterio

    for attempt in range(tries):
        try:
            return raster.read(1, window=window, out_dtype=np.float32)
        except rasterio.errors.RasterioIOError:
            if attempt == tries - 1:
                raise
            time.sleep(2 ** attempt)


def pixel_digest(bands):
    """The SHA-256 of the pixels of a tile: band-major, row-major, little-endian `uint16`."""
    return hashlib.sha256(np.ascontiguousarray(bands.astype("<u2")).tobytes()).hexdigest()


def write_tile(path, x, y, bands):
    import rasterio
    from rasterio.transform import from_origin

    path.parent.mkdir(parents=True, exist_ok=True)
    west, top = tile_origin(x, y)
    profile = {"driver": "GTiff", "width": SIZE, "height": SIZE, "count": len(BANDS), "dtype": "uint16",
               "crs": "EPSG:3857", "transform": from_origin(west, top, CELL, CELL), "nodata": NODATA,
               "compress": "deflate", "predictor": 2, "tiled": True, "blockxsize": SIZE, "blockysize": SIZE}
    part = path.with_name(path.name + ".part")
    with rasterio.open(part, "w", **profile) as tile:
        tile.write(bands)
        tile.descriptions = BANDS
    os.replace(part, path)


TILES = {"ch": tile_ch}


def command_fetch(args):
    """One tile of `forest-reference/<key>` into `--out`: `12/<x>/<y>.tif`, then
    `12-<x>-<y>.sha256`, the digest of its pixels; or the empty `12-<x>-<y>.none` where the source
    has no pixel in the tile."""
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    stop_with_parent()
    z, x, y = map(int, args.tile.split("-"))
    if z != ZOOM or not (0 <= x < 1 << z and 0 <= y < 1 << z):
        raise SystemExit(f"--tile {args.tile} is 12-<x>-<y>, a zoom 12 tile")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    bands = TILES[args.source](x, y)
    if bands is None:
        (out / f"{args.tile}.none").touch()
        return 0
    write_tile(out / f"12/{x}/{y}.tif", x, y, bands)
    (out / f"{args.tile}.sha256").write_text(pixel_digest(bands) + "\n")
    return 0


def command_index_tiles(args):
    """`index.json` of a version: each tile of `--digests`, a JSON object of `<x>/<y>` to the pixel
    digest that `fetch` wrote."""
    digests = dict(sorted(json.loads(Path(args.digests).read_text()).items()))
    index = {"schema": 1, "zoom": ZOOM, "size": SIZE, "bands": list(BANDS), "source": args.source,
             "fetched": args.fetched, "sha256": digests}
    Path(args.out).write_text(json.dumps(index, indent=2, sort_keys=True) + "\n")
    return 0


def stop_with_parent():
    """Stop when the process `OBC_PARENT_PID` names ends, so no orphan writes into a later run."""
    parent = int(os.environ.get("OBC_PARENT_PID") or 0)

    def watch():
        while True:
            time.sleep(2)
            try:
                os.kill(parent, 0)
            except ProcessLookupError:
                os.kill(os.getpid(), signal.SIGTERM)
                return

    if parent:
        threading.Thread(target=watch, daemon=True).start()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    fetch = commands.add_parser("fetch")
    fetch.add_argument("source", choices=sorted(TILES))
    fetch.add_argument("--tile", required=True)
    fetch.add_argument("--out", required=True)
    index = commands.add_parser("index-tiles")
    index.add_argument("source", choices=sorted(TILES))
    index.add_argument("--fetched", required=True)
    index.add_argument("--digests", required=True)
    index.add_argument("--out", required=True)
    args = parser.parse_args()
    return {"fetch": command_fetch, "index-tiles": command_index_tiles}[args.command](args)


if __name__ == "__main__":
    raise SystemExit(main())
