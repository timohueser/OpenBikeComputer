"""The blocks of the prepared source `forest`, as specs/obc-data.md defines them: per
cell of a zoom 12 tile at 256 px, the canopy height, the cover, the stems per hectare and the conifer
share, 4 x 4 tiles to a zoom 10 block.

    python -m tools.forest_reference fetch 12-2143-1443=ch,global --block 10-535-360 --out DIR

`fetch` reads the source rasters over HTTP and keeps no raw raster on disk.
"""

import argparse
import hashlib
import json
import math
import os
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from numba import njit

ZOOM, SIZE = 12, 256
WORLD = 2 * math.pi * 6378137
CELL = WORLD / (SIZE << ZOOM)
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
# Canopy Height Maps v2 of Meta and WRI: a COG per zoom 10 tile, named by its quadkey, at the pixels of zoom 17.
CHM = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v2/global/dinov3_global_chm_v2_ml3/chm"
# Its heights read low and its tree tops are artefacts. Fitted against `canopy-ch` on the canopy cells of eight
# tiles of the Jura, the Mittelland and the Alps: the height of a cell is `CHM_SCALE` times the height of the
# model plus `CHM_OFFSET_M`, and its stems fill `STEMS_SHARE` of its cover with crowns of its height.
CHM_SCALE, CHM_OFFSET_M, STEMS_SHARE = 0.982, 3.73, 0.417
# Dominant Leaf Type of CLMS on CDSE S3, in tiles of 100 km in EPSG:3035 named by their south-west corner.
DLT_YEAR = 2024
DLT = ("/vsis3/eodata/CLMS/landcover_landuse/dominant_leaf_type/clms_vlcc_dominant-leaf-type_europe_10m_yearly_v1/"
       f"{DLT_YEAR}/01/01/{{name}}/{{name}}.tif")
# The DLT tiles that CDSE S3 answered NoSuchKey. GDAL caches the miss and reports a later open of the
# path as a plain HTTP 404, which a wrong bucket also gives.
ABSENT = set()


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
                if not (di < 0 or di == 0 and dj < 0) or not (0 <= r < rows and 0 <= c < cols):
                    continue
                q = r * cols + c
                if parent[q] >= 0 and level[r, c] == level[i, j]:
                    a, b = root(parent, p), root(parent, q)
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
def lattice_cells(lattice_j, lattice_i, at_rows, at_cols, shape):
    """The cell of each pixel from the cell coordinates of the lattice points at every 64th row
    `at_rows` and column `at_cols` and one past the last, bilinear in between: -1 outside the
    `SIZE` × `SIZE` cells of the tile."""
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
            if 0 <= i < SIZE and 0 <= j < SIZE:
                cells[r, c] = int(i) * SIZE + int(j)
    return cells


def pixel_cells(x, y, rows, cols, transform):
    """The cell of tile x/y of each pixel of the raster rows `rows` and columns `cols` (LV95 grid
    `transform`), or -1. The exact projection runs on a lattice of 64 pixels; in between, the
    projection is affine to within millimetres."""
    from rasterio.warp import transform as project

    height, width = rows[1] - rows[0], cols[1] - cols[0]
    at_rows, at_cols = np.r_[np.arange(0, height, 64), height], np.r_[np.arange(0, width, 64), width]
    grid_rows, grid_cols = np.meshgrid(at_rows + rows[0] + 0.5, at_cols + cols[0] + 0.5, indexing="ij")
    east, north = transform * (grid_cols, grid_rows)
    mx, my = (np.reshape(values, east.shape) for values in project("EPSG:2056", "EPSG:3857", east.ravel(), north.ravel()))
    west, top = tile_origin(x, y)
    return lattice_cells((mx - west) / CELL, (top - my) / CELL, at_rows, at_cols, (height, width))


def totals(count):
    """Empty sums of `count` cells for `accumulate`: the known pixels, the canopy pixels, the tree
    tops and the histogram of the canopy heights in decimetres."""
    return (np.zeros(count, np.int64), np.zeros(count, np.int64), np.zeros(count, np.int64),
            np.zeros((count, TALLEST_DM + 1), np.int32))


@njit(nogil=True, cache=True)
def accumulate(cells, heights, tops, pixels, crowns, stems, histogram):
    """Add each pixel to the sums of its cell in `cells`; -1 is no cell."""
    tallest = histogram.shape[1] - 1
    for r in range(cells.shape[0]):
        for c in range(cells.shape[1]):
            cell, height = cells[r, c], heights[r, c]
            if cell < 0 or math.isnan(height):
                continue
            pixels[cell] += 1
            stems[cell] += tops[r, c]
            if height >= CANOPY_M:
                crowns[cell] += 1
                histogram[cell, min(int(height * 10), tallest)] += 1


@njit(nogil=True, cache=True)
def percentile_95(crowns, histogram):
    """The lower nearest-rank 95th percentile of the histogram of each cell."""
    out = np.zeros(len(crowns), np.int64)
    for k in range(len(crowns)):
        rank, seen = math.floor(0.95 * (crowns[k] - 1)), 0
        for value in range(histogram.shape[1]):
            seen += histogram[k, value]
            if seen > rank:
                out[k] = value
                break
    return out


def cell_stats(sums, pixel_m=0.5):
    """The height, cover and stems bands of the cells of `sums`. A cell without a known pixel stays
    `NODATA`."""
    pixels, crowns, stems, histogram = sums
    p95 = percentile_95(crowns, histogram)
    out = np.full((3, len(pixels)), NODATA, np.uint16)
    seen = pixels > 0
    hectares = pixels[seen] * pixel_m * pixel_m / 1e4
    out[0, seen] = np.where(crowns[seen] > 0, p95[seen], 0)
    out[1, seen] = np.rint(100 * crowns[seen] / pixels[seen])
    out[2, seen] = np.minimum(np.rint(stems[seen] / hectares), NODATA - 1)
    return out


def tile_origin(x, y):
    """The west and north edges of tile x/y in Web Mercator metres."""
    return x * SIZE * CELL - WORLD / 2, WORLD / 2 - y * SIZE * CELL


def http(url, delays=(5, 30, 120, 600)):
    """The body of `url`. The waits ride out an outage of a service of minutes, as one failed tile
    ends the whole prepare."""
    for delay in (*delays, None):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "openbikecomputer"})
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.read()
        except OSError:
            if delay is None:
                raise
            time.sleep(delay)


def vhm_years():
    """The rasters of the vegetation height model, newest acquisition first."""
    items = json.loads(http(f"{VHM}?limit=100"))["features"]
    hrefs = [(item["properties"]["datetime"], asset["href"]) for item in items
             for asset in item["assets"].values() if asset["href"].endswith(".tif")]
    return [href for _, href in sorted(hrefs, reverse=True)]


def leaf_type(x, y):
    """The conifer share of the tree pixels of each cell, in %, from Dominant Leaf Type `DLT_YEAR`
    (1 broadleaved, 2 coniferous) at four sub-pixels per cell side; `NODATA` without a tree pixel."""
    import rasterio
    from rasterio.enums import Resampling
    from rasterio.errors import RasterioIOError
    from rasterio.transform import from_origin
    from rasterio.vrt import WarpedVRT
    from rasterio.warp import transform_bounds

    from . import planner_snow

    planner_snow.cdse_credentials()
    west, top = tile_origin(x, y)
    span, side = SIZE * CELL, 4 * SIZE
    left, bottom, right, upper = transform_bounds("EPSG:3857", "EPSG:3035", west, top - span, west + span, top)
    grid = {"crs": "EPSG:3857", "transform": from_origin(west, top, span / side, span / side), "width": side,
            "height": side, "resampling": Resampling.nearest, "nodata": 255}
    classes = np.full((side, side), 255, np.uint8)

    def read(name):
        """The classes of tile `name` on the grid, or None where CLMS publishes none. The waits
        ride out a refused connection, which the GDAL retries do not cover."""
        if name in ABSENT:
            return None
        for delay in (5, 30, 120, 600, None):
            try:
                with rasterio.open(DLT.format(name=name)) as source, WarpedVRT(source, **grid) as warped:
                    return warped.read(1)
            except RasterioIOError as error:
                # CLMS publishes no tile without land; CDSE S3 answers NoSuchKey.
                if "NoSuchKey" in str(error):
                    ABSENT.add(name)
                    return None
                if delay is None:
                    raise
                time.sleep(delay)

    # The tile programs together open more connections than the 4 that CDSE allows a user: GDAL
    # retries the HTTP status of that refusal.
    with rasterio.Env(**{**GDAL, "GDAL_HTTP_MAX_RETRY": 8, "GDAL_HTTP_RETRY_DELAY": 3}):
        for e in range(int(left // 100_000), int(right // 100_000) + 1):
            for n in range(int(bottom // 100_000), int(upper // 100_000) + 1):
                part = read(f"CLMS_HRLVLCC_DLT_S{DLT_YEAR}_R10m_E{e:02d}N{n:02d}_03035_V01_R00")
                if part is not None:
                    classes = np.where(part != 255, part, classes)
    classes = classes.reshape(SIZE, 4, SIZE, 4)
    broad, conifer = ((classes == value).sum(axis=(1, 3)) for value in (1, 2))
    trees = broad + conifer
    return np.where(trees > 0, np.rint(100 * conifer / np.maximum(trees, 1)), NODATA).astype(np.uint16)


GDAL = {"GDAL_CACHEMAX": 512, "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR", "GDAL_HTTP_MAX_RETRY": 5,
        "GDAL_HTTP_RETRY_DELAY": 2, "GDAL_HTTP_TIMEOUT": 120, "GDAL_NUM_THREADS": 4, "GDAL_HTTP_MULTIPLEX": "YES"}


def tile_ch(x, y):
    """The height, cover and stems bands of tile x/y from the newest acquisition of the vegetation
    height model LiDAR NFI at each pixel, or None where it has no pixel."""
    import rasterio
    from rasterio.warp import transform_bounds

    def opened(href):
        """The raster, and where it has data: the valid pixels of its overview of 256 m, grown by
        one, and the size of an overview pixel in raster rows and columns."""
        with rasterio.Env(**GDAL):
            raster = rasterio.open(f"/vsicurl/{href}")
            with rasterio.open(f"/vsicurl/{href}", overview_level=COARSE) as overview:
                valid = overview.read(1) != overview.nodata
        padded, grown = np.pad(valid, 1), np.zeros_like(valid)
        for di in range(3):
            for dj in range(3):
                grown |= padded[di:di + valid.shape[0], dj:dj + valid.shape[1]]
        return raster, (grown, raster.height / valid.shape[0], raster.width / valid.shape[1])

    def read(rows):
        with rasterio.Env(**GDAL):
            heights = read_newest(rasters, coverage, rows, cols)
        return np.full((rows[1] - rows[0], cols[1] - cols[0]), np.nan, np.float32) if heights is None else heights

    # Each open waits for the header of a remote file, so they wait together.
    with ThreadPoolExecutor(8) as pool:
        rasters, coverage = zip(*pool.map(opened, vhm_years()))
    try:
        grid = rasters[0]
        if any(raster.transform != grid.transform or raster.shape != grid.shape for raster in rasters):
            raise ValueError("the acquisitions of the vegetation height model have different grids")
        pixel_m, margin, (west, top) = grid.res[0], halo(grid.res[0]), tile_origin(x, y)
        left, bottom, right, upper = transform_bounds(
            "EPSG:3857", "EPSG:2056", west, top - SIZE * CELL, west + SIZE * CELL, top, densify_pts=64)
        (r0, r1), (c0, c1) = (sorted(values) for values in rasterio.transform.rowcol(
            grid.transform, [left, right], [upper, bottom], op=math.floor))
        rows = (max(r0 - margin, 0), min(r1 + 1 + margin, grid.height))
        cols = (max(c0 - margin, 0), min(c1 + 1 + margin, grid.width))
        if rows[0] >= rows[1] or cols[0] >= cols[1]:
            return None
        sums = canopy_sums(read, rows, cols, grid.block_shapes[0][0], pixel_m,
                           lambda span: pixel_cells(x, y, span, cols, grid.transform))
    finally:
        for raster in rasters:
            raster.close()
    return None if sums is None else cell_stats(sums, pixel_m).reshape(3, SIZE, SIZE)


def canopy_sums(read, rows, cols, block, pixel_m, cells, tops=True):
    """The sums of `accumulate` of the raster rows `rows` and columns `cols` of a canopy height
    model, or None where it has no pixel. `read(rows)` gives the heights of rows, NaN unknown, and
    `cells(rows)` the cell of each of their pixels in `cols`; without `tops`, no tree top counts. The
    rows are read in strips of whole
    blocks of `block` rows, so each block downloads once; one thread reads the next strip while the
    other reduces this one."""
    margin = halo(pixel_m) if tops else 0
    edges = [rows[0], *range((rows[0] // block + 1) * block, rows[1], block), rows[1]]
    strips = list(zip(edges, edges[1:]))
    sums, seen, done = totals(SIZE * SIZE), False, rows[0]
    context = np.empty((0, cols[1] - cols[0]), np.float32)
    with ThreadPoolExecutor(1) as reader:
        pending = reader.submit(read, strips[0])
        for k, (_, end) in enumerate(strips):
            heights = np.vstack([context, pending.result()])
            if k + 1 < len(strips):
                pending = reader.submit(read, strips[k + 1])
            first = end - len(heights)
            # The tree tops of a row are final once the `margin` rows after it are read.
            last = end if end == rows[1] else end - margin
            if last > done and not np.isnan(heights).all():
                seen = True
                part = heights[done - first:last - first]
                found = tree_tops(heights, pixel_m)[done - first:last - first] if tops else np.zeros(part.shape, np.bool_)
                accumulate(cells((done, last)), part, found, *sums)
            done, context = max(done, last), heights[len(heights) - 2 * margin:]
    return sums if seen else None


def tile_global(x, y):
    """The height, cover and stems bands of tile x/y from Canopy Height Maps v2, whole metres where
    0 is also no data, or None where it has no file. Its pixels lie on the cells: 32 × 32 to a cell.
    The height and the stems are corrected as `CHM_SCALE` says."""
    import rasterio
    from rasterio.errors import RasterioIOError
    from rasterio.windows import Window

    qx, qy, side = x >> 2, y >> 2, 32 * SIZE
    quadkey = "".join(str((qx >> i & 1) | (qy >> i & 1) << 1) for i in range(ZOOM - 3, -1, -1))
    with rasterio.Env(**GDAL):
        try:
            raster = rasterio.open(f"/vsicurl/{CHM}/{quadkey}.tif")
        except RasterioIOError as error:
            # The model has no file of a tile without land.
            if "404" in str(error):
                return None
            raise
        with raster:
            west, top = tile_origin(qx << 2, qy << 2)
            if raster.crs != "EPSG:3857" or raster.shape != (4 * side, 4 * side) or not np.allclose(
                    (raster.transform.a, -raster.transform.e, raster.transform.c, raster.transform.f),
                    (CELL / 32, CELL / 32, west, top)):
                raise ValueError(f"{quadkey}.tif is not on the pixels of zoom 17 of its zoom 10 tile")
            r0, c0 = (y & 3) * side, (x & 3) * side
            rows, cols = (r0, r0 + side), (c0, c0 + side)
            columns = np.arange(SIZE, dtype=np.int32).repeat(32)

            def read(span):
                return read_window(raster, Window(c0, span[0], side, span[1] - span[0]))

            def cells(span):
                return ((np.arange(*span, dtype=np.int32) - r0) // 32 * SIZE)[:, None] + columns

            sums = canopy_sums(read, rows, cols, raster.block_shapes[0][0], CELL / 32, cells, tops=False)
    if sums is None:
        return None
    height, cover, _ = cell_stats(sums, CELL / 32).reshape(3, SIZE, SIZE).astype(np.float64)
    height = np.where(height > 0, np.minimum(np.rint(10 * (CHM_SCALE * height / 10 + CHM_OFFSET_M)), TALLEST_DM), 0)
    crown = np.vectorize(crown_radius)(height / 10, 1.0)
    stems = np.where(height > 0, np.rint(STEMS_SHARE * cover / 100 * 1e4 / (np.pi * crown ** 2)), 0)
    return np.stack([height, cover, stems]).astype(np.uint16)


def read_newest(rasters, coverage, rows, cols):
    """The heights of the raster rows `rows` and columns `cols` from the first of `rasters` that
    has each pixel, NaN where none has it, or None when none has a pixel. A raster is read only
    over the rows and columns that are still unknown and where `coverage`, its valid overview
    pixels and their size in raster rows and columns, shows data."""
    from rasterio.windows import Window

    heights = np.full((rows[1] - rows[0], cols[1] - cols[0]), np.nan, np.float32)
    for raster, (valid, along_rows, along_cols) in zip(rasters, coverage):
        unknown = np.isnan(heights)
        if not unknown.any():
            break
        at_rows, at_cols = np.flatnonzero(unknown.any(axis=1)), np.flatnonzero(unknown.any(axis=0))
        r0, r1, c0, c1 = rows[0] + at_rows[0], rows[0] + at_rows[-1] + 1, cols[0] + at_cols[0], cols[0] + at_cols[-1] + 1
        k0, m0 = int(r0 // along_rows), int(c0 // along_cols)
        near = valid[k0:int((r1 - 1) // along_rows) + 1, m0:int((c1 - 1) // along_cols) + 1]
        if not near.any():
            continue
        has_rows, has_cols = np.flatnonzero(near.any(axis=1)) + k0, np.flatnonzero(near.any(axis=0)) + m0
        r0, r1 = max(r0, math.floor(has_rows[0] * along_rows)), min(r1, math.ceil((has_rows[-1] + 1) * along_rows))
        c0, c1 = max(c0, math.floor(has_cols[0] * along_cols)), min(c1, math.ceil((has_cols[-1] + 1) * along_cols))
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
    """The SHA-256 of the pixels of a block: band-major, row-major, little-endian `uint16`."""
    return hashlib.sha256(np.ascontiguousarray(bands.astype("<u2")).tobytes()).hexdigest()


def write_block(path, x, y, bands):
    """The block `x`, `y` of zoom 10: its zoom 12 tiles are its internal tiles, and GDAL leaves out
    an internal tile without a known cell."""
    import rasterio
    from rasterio.transform import from_origin

    path.parent.mkdir(parents=True, exist_ok=True)
    west, top = tile_origin(4 * x, 4 * y)
    profile = {"driver": "GTiff", "width": 4 * SIZE, "height": 4 * SIZE, "count": len(BANDS), "dtype": "uint16",
               "crs": "EPSG:3857", "transform": from_origin(west, top, CELL, CELL), "nodata": NODATA,
               "compress": "deflate", "predictor": 2, "tiled": True, "blockxsize": SIZE, "blockysize": SIZE,
               "sparse_ok": True}
    part = path.with_name(path.name + ".part")
    with rasterio.open(part, "w", **profile) as block:
        block.write(bands)
        block.descriptions = BANDS
    os.replace(part, path)


def in_block(x, y):
    """The cells of zoom 12 tile `x`, `y` in the bands of its block."""
    return np.s_[:, y % 4 * SIZE:(y % 4 + 1) * SIZE, x % 4 * SIZE:(x % 4 + 1) * SIZE]


TILES = {"ch": tile_ch, "global": tile_global}


def tile_bands(x, y, models):
    """The bands of zoom 12 tile `x`, `y` from the canopy models `models` in order: a cell takes the
    first model that has a pixel in it."""
    bands = np.full((4, SIZE, SIZE), NODATA, np.uint16)
    for key in models:
        part = TILES[key](x, y)
        if part is not None:
            unknown = bands[0] == NODATA
            bands[:3, unknown] = part[:, unknown]
    # A tile without a canopy needs no leaf type, which would ask CDSE for nothing.
    if ((bands[0] != NODATA) & (bands[0] > 0)).any():
        bands[3] = np.where(bands[0] != NODATA, leaf_type(x, y), NODATA)
    return bands


def command_fetch(args):
    """One block of `forest` into `--out`: each tile `12-<x>-<y>=<key>,…` of `--block 10-<x>-<y>`
    from its canopy models, written to `10/<x>/<y>.tif`, then `10-<x>-<y>.sha256`, the digest of its
    pixels. A tile that the arguments do not name is unknown."""
    z, bx, by = map(int, args.block.split("-"))
    if z != 10 or not (0 <= bx < 1 << z and 0 <= by < 1 << z):
        raise SystemExit(f"--block {args.block} is 10-<x>-<y>, a zoom 10 tile")
    bands = np.full((4, 4 * SIZE, 4 * SIZE), NODATA, np.uint16)
    for text in args.tiles:
        tile, _, keys = text.partition("=")
        z, x, y = map(int, tile.split("-"))
        if z != ZOOM or (x >> 2, y >> 2) != (bx, by) or not keys:
            raise SystemExit(f"{text} is not 12-<x>-<y>=<keys> of a tile of block {args.block}")
        bands[in_block(x, y)] = tile_bands(x, y, keys.split(","))
    out = Path(args.out)
    write_block(out / f"10/{bx}/{by}.tif", bx, by, bands)
    (out / f"{args.block}.sha256").write_text(pixel_digest(bands) + "\n")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    fetch = commands.add_parser("fetch")
    fetch.add_argument("tiles", nargs="+", help="12-<x>-<y>=<models>: a tile and its canopy models in order, "
                                                f"of {', '.join(sorted(TILES))}")
    fetch.add_argument("--block", required=True)
    fetch.add_argument("--out", required=True)
    return command_fetch(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
