"""Bake the planner snow layer of one region: `snow.pmtiles`, as `specs/planner-snow-tiles.md` defines it.

The `obc data` step (`--step`) reads HR-WSI where it has data and MODIS elsewhere. The command line
reads one source:

    uv run --locked --group planner-snow python -m tools.planner_snow REGION

The default source is NASA MODIS daily snow cover from the anonymous Microsoft Planetary Computer
copy. It ends in June 2025. The bake reads only the region window of each daily file and writes no
raw files. Seasons after June 2025 are not supported yet.

`--source copernicus-hr-wsi` reads the HR-WSI Snow Phenology S2 yearly rasters (20 m) instead. It
reads the window of one zoom-9 tile at a time from each file on the Copernicus Data Space S3 endpoint
`https://eodata.dataspace.copernicus.eu`, with `CDSE_S3_ACCESS_KEY` and `CDSE_S3_SECRET_KEY` in
`~/.config/openbikecomputer/cdse-s3.env`.
"""

import argparse
from collections import deque, namedtuple
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import gzip
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

from . import planner_geo as geo, step_request

NO_SNOW, FULL, NO_DATA = 253, 254, 255
LAST_STEP = 182
TILE = 256
# `smooth`: 20 m day values are noisy, so the lower zooms smooth them before they blend.
SOURCES = {
    "nasa-modis": {"resolution_m": 500, "smooth": False},
    "copernicus-hr-wsi": {"resolution_m": 20, "smooth": True},
}
CANOPY = "https://storage.googleapis.com/earthenginepartners-hansen/GFC-2023-{0}/Hansen_GFC-2023-{0}_treecover2000_{1}.tif"
# MODIS sees the canopy, not the snow under it; HR-WSI corrects for trees. The owner chose 75 % canopy cover as
# "dense": below it, MODIS still sees the snow between the trees.
DENSE_CANOPY_PERCENT = 75
STAC = "https://planetarycomputer.microsoft.com/api/stac/v1/search"
SAS = "https://planetarycomputer.microsoft.com/api/sas/v1/token/{}/{}"
OVERPASS = "https://overpass-api.de/api/interpreter"
ODATA = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products"
COPERNICUS_PRODUCT = "CLMS_WSI_SP_020m_"
CDSE_KEYS = Path.home() / ".config/openbikecomputer/cdse-s3.env"
USER_AGENT = "OpenBikeComputer planner snow bake (https://github.com/timohueser/OpenBikeComputer)"
# The MODIS sinusoidal grid: 36 × 18 tiles of 2400 × 2400 pixels.
SINUSOIDAL = "+proj=sinu +lon_0=0 +x_0=0 +y_0=0 +R=6371007.181 +units=m +no_defs"
MODIS_TILE_M, MODIS_PIXELS = 1111950.5197665233, 2400
MODIS_X0, MODIS_Y0 = -20015109.355798, 10007554.677899
MODIS_SNOW_NDSI = 10
# The bake reads the source one tile of this zoom at a time, so memory stays small for a large region.
CHUNK_ZOOM = 9

Grid = namedtuple("Grid", "crs transform shape")


def max_zoom(resolution_m, bounds):
    """The zoom whose pixel size at the middle latitude of the bounds is nearest to the source resolution, in log scale."""
    metres = 2 * math.pi * 6378137 * math.cos(math.radians((bounds[1] + bounds[3]) / 2)) / TILE
    return min(range(23), key=lambda z: abs(math.log(metres / 2 ** z / resolution_m)))


def season_start(first_season, season):
    """Day of 1 September of `season`, counted from 1 September of `first_season`."""
    return (dt.date(season, 9, 1) - dt.date(first_season, 9, 1)).days


def encode(onset, length, days, data):
    """Onset and melt-out bytes (2, ...) of the longest snow period: its first day index and its length."""
    meltout = onset + length - 1
    dated = np.stack([np.clip(onset, 0, 2 * LAST_STEP) // 2, np.clip(meltout, 0, 2 * LAST_STEP) // 2])
    value = np.where(length >= days, FULL, np.where(length == 0, NO_SNOW, dated))
    return np.where(data, value, NO_DATA).astype(np.uint8)


class SnowSeasons:
    """The longest snow period of each season, from daily observations fed one day at a time.

    Days count from 1 September of the first season. A day without a clear observation takes the
    state of the nearest clear day; on a tie, the earlier one.
    """

    def __init__(self, first_season, seasons, shape):
        self.bounds = np.array([season_start(first_season, first_season + k) for k in range(seasons + 1)])
        self.day = -1
        self.last = np.full(shape, -1, np.int32)  # day of the last clear observation
        self.snow = np.zeros(shape, bool)  # snow on that day
        self.start = np.zeros(shape, np.int32)  # first day of the open snow period
        self.clear = np.zeros((seasons, *shape), np.int32)
        self.length = np.zeros((seasons, *shape), np.int32)
        self.onset = np.zeros((seasons, *shape), np.int32)

    def observe(self, day, clear, snow):
        if not self.day < day < self.bounds[-1]:
            raise ValueError("Days must increase and stay inside the seasons")
        self.day = day
        self.clear[np.searchsorted(self.bounds, day, "right") - 1] += clear
        snow = clear & snow
        switch = (self.last + day) // 2 + 1  # the first gap day nearer to today than to the last clear day
        self._credit(clear & self.snow & ~snow, self.start, switch - 1)
        self.start = np.where(snow & ~self.snow, np.where(self.last < 0, 0, switch), self.start)
        self.last = np.where(clear, day, self.last)
        self.snow = np.where(clear, snow, self.snow)

    def _credit(self, mask, first, last):
        """Offer the snow period from `first` to `last` (inclusive) to each season it covers."""
        if not mask.any():
            return
        low = np.searchsorted(self.bounds, first[mask].min(), "right") - 1
        high = min(np.searchsorted(self.bounds, last[mask].max(), "right") - 1, len(self.length) - 1)
        for k in range(low, high + 1):
            a, b = np.maximum(first, self.bounds[k]), np.minimum(last, self.bounds[k + 1] - 1)
            longer = mask & (b - a + 1 > self.length[k])
            self.length[k] = np.where(longer, b - a + 1, self.length[k])
            self.onset[k] = np.where(longer, a - self.bounds[k], self.onset[k])

    def planes(self, end):
        """Season planes (seasons, 2, ...); `end` is the first day after the source."""
        end = min(end, self.bounds[-1])
        self._credit(self.snow, self.start, np.full_like(self.start, end - 1))
        planes = []
        for k in range(len(self.length)):
            data = self.clear[k] > 0
            if end < self.bounds[k + 1]:
                # The open snow period may continue after the source ends and become the longest.
                possible = self.bounds[k + 1] - np.maximum(self.start, self.bounds[k])
                data &= ~(self.snow & (possible > self.length[k]))
            planes.append(encode(self.onset[k], self.length[k], self.bounds[k + 1] - self.bounds[k], data))
        return np.stack(planes)


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


def canopy_tiles(bounds):
    """Names of the 10° Hansen tiles, named by their north-west corner, that the bounds overlap with one MODIS pixel of
    margin: the bake samples the MODIS pixels around its edge. As `canopy_tiles` of the planner step list."""
    pad = 10 / MODIS_PIXELS
    widen = pad / math.cos(math.radians(max(abs(bounds[1]), abs(bounds[3]))))
    west, south, east, north = bounds[0] - widen, bounds[1] - pad, bounds[2] + widen, bounds[3] + pad
    return [f"{abs(lat):02d}{'N' if lat >= 0 else 'S'}_{abs(lon):03d}{'E' if lon >= 0 else 'W'}"
            for lat in range(math.floor(south / 10) * 10 + 10, math.ceil(north / 10) * 10 + 1, 10)
            for lon in range(math.floor(west / 10) * 10, math.ceil(east / 10) * 10, 10)]


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


def overpass_trails(bounds):
    """The Overpass answer, in JSON, with the OSM `highway=path|track` ways that the bounds touch."""
    west, south, east, north = bounds
    query = f'[out:json][timeout:180];way["highway"~"^(path|track)$"]({south},{west},{north},{east});out geom;'
    request = urllib.request.Request(OVERPASS, data=urllib.parse.urlencode({"data": query}).encode(),
                                     headers={"User-Agent": USER_AGENT})
    for attempt in range(4):
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            # Overpass answers 429 and 504 when it is busy.
            if error.code not in (429, 504) or attempt == 3:
                raise
            time.sleep(60 * (attempt + 1))


def bake(sources, first_season, seasons, bounds, output, credit, workers=4):
    """Write the archive with the attribution `credit`; return the tile count.

    `sources` is [(name, planes)], the finest first: `planes(bounds)` gives the season planes and their grid around
    the bounds of one chunk tile. In each season, a pixel takes the first source that has data there. The finest
    source sets the max zoom, the smoothing and the metadata. The chunks bake in `workers` threads: numpy, zlib and
    GDAL release the GIL. CDSE S3 allows 4 connections per user.
    """
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    name = sources[0][0]
    top = max_zoom(SOURCES[name]["resolution_m"], bounds)
    chunk = min(CHUNK_ZOOM, top)
    west, south, east, north = bounds
    tiles = {}

    def area(z, x, y):
        """The part of the bounds in tile z/x/y, or None."""
        w, s, e, n_ = geo.tile_bounds(z, x, y)
        if w >= east or e <= west or s >= north or n_ <= south:
            return None
        return max(w, west), max(s, south), min(e, east), min(n_, north)

    def build(z, x, y, read=None):
        """The body of tile z/x/y from `read`, the planes and grids of the sources around its chunk. Its tiles and
        those below go into `tiles`."""
        if area(z, x, y) is None:
            return None
        if z == chunk and read is None:
            return chunks[x, y].result()
        if z == top:
            body = None
            for planes, grid in read:
                if body is not None and not (body == NO_DATA).any():
                    break
                sampled = sample(planes, grid, z, x, y)
                body = sampled if body is None else np.where(body == NO_DATA, sampled, body)
            lon_c, lat_c = tile_lonlat(z, x, y)
            outside = ((lon_c < west) | (lon_c > east) | (lat_c < south) | (lat_c > north)).reshape(TILE, TILE)
            body[:, :, outside] = NO_DATA
        else:
            children = {(dx, dy): build(z + 1, 2 * x + dx, 2 * y + dy, read) for dx in (0, 1) for dy in (0, 1)}
            body = parent(children, seasons, SOURCES[name]["smooth"])
        if (body != NO_DATA).any():
            tiles[zxy_to_tileid(z, x, y)] = gzip.compress(body.tobytes(), mtime=0)
        return body

    x0, y0 = geo.mercator(west, north, chunk)
    x1, y1 = geo.mercator(east, south, chunk)
    pool = ThreadPoolExecutor(workers)
    try:
        read = lambda part: [planes(part) for _, planes in sources]
        chunks = {(x, y): pool.submit(lambda x, y, part: build(chunk, x, y, read(part)), x, y, part)
                  for x in range(int(x0), int(x1) + 1)
                  for y in range(int(y0), int(y1) + 1) if (part := area(chunk, x, y))}
        build(0, 0, 0)
    finally:
        pool.shutdown(cancel_futures=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    e7 = lambda value: round(value * 1e7)
    with tempfile.NamedTemporaryFile(dir=output.parent, prefix=".snow-", delete=False) as stream:
        try:
            writer = Writer(stream)
            for tile_id in sorted(tiles):
                writer.write_tile(tile_id, tiles[tile_id])
            meta = SOURCES[name]
            writer.finalize({
                "tile_type": TileType.UNKNOWN, "tile_compression": Compression.GZIP,
                "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                "center_zoom": top, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
            }, {
                "first_season": first_season, "seasons": seasons, "step_days": 2, "source": name,
                "resolution_m": meta["resolution_m"], "attribution": credit,
            })
            stream.flush()
            os.replace(stream.name, output)
        except BaseException:
            os.unlink(stream.name)
            raise
    return len(tiles)


def http_json(url, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json", "User-Agent": USER_AGENT})
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


def modis_items(bounds, start, end):
    """{date: [(platform, href)]} of the MOD10A1/MYD10A1 files from `start` to `end`."""
    body = {"collections": ["modis-10A1-061"], "bbox": list(bounds), "datetime": f"{start}/{end}", "limit": 1000}
    items, page = {}, http_json(STAC, body)
    while True:
        for feature in page["features"]:
            items.setdefault(dt.date.fromisoformat(feature["properties"]["start_datetime"][:10]), []).append(
                (feature["id"][:3], feature["assets"]["NDSI_Snow_Cover"]["href"]))
        link = next((link for link in page.get("links", []) if link["rel"] == "next"), None)
        if link is None:
            break
        page = http_json(link["href"], link.get("body")) if link.get("method") == "POST" else http_json(link["href"])
    # Each file needs a read token of its storage account and container; a token is valid for about an hour.
    container = lambda href: (urllib.parse.urlsplit(href).netloc.split(".")[0], urllib.parse.urlsplit(href).path.split("/")[1])
    keys = {container(href) for files in items.values() for _, href in files}
    tokens = {key: http_json(SAS.format(*key))["token"] for key in keys}
    return {day: [(platform, f"{href}?{tokens[container(href)]}") for platform, href in files] for day, files in items.items()}


def modis_day(files, grid):
    """Clear and snow masks of one day from its files [(platform, path)]: Terra first, Aqua where Terra is not
    clear. A file is a MODIS tile or a window of one; its transform places it on the grid."""
    import rasterio
    from rasterio.windows import Window

    rows, cols = grid.shape
    size = grid.transform.a
    ndsi = {"MOD": np.full(grid.shape, NO_DATA, np.uint8), "MYD": np.full(grid.shape, NO_DATA, np.uint8)}
    for platform, path in files:
        for attempt in range(5):
            try:
                with rasterio.open(path) as src:
                    col, row = (src.transform.c - grid.transform.c) / size, (grid.transform.f - src.transform.f) / size
                    if abs(src.transform.a - size) > 1e-3 or max(abs(col - round(col)), abs(row - round(row))) * size > 1:
                        raise ValueError(f"{path} is not on the MODIS sinusoidal grid")
                    col, row = round(col), round(row)
                    top, left = max(row, 0), max(col, 0)
                    bottom, right = min(row + src.height, rows), min(col + src.width, cols)
                    if top < bottom and left < right:
                        window = Window(left - col, top - row, right - left, bottom - top)
                        ndsi[platform][top:bottom, left:right] = src.read(1, window=window)
                break
            except rasterio.errors.RasterioIOError:
                if attempt == 4:
                    raise
                time.sleep(3 * (attempt + 1))
    # 0–100 is NDSI × 100 and 254 a saturated detector; the other values are cloud, night, water or fill.
    clear = lambda value: (value <= 100) | (value == 254)
    value = np.where(clear(ndsi["MOD"]), ndsi["MOD"], ndsi["MYD"])
    return clear(value), clear(value) & (value >= MODIS_SNOW_NDSI)


def modis_seasons(bounds, first_season, last_season):
    """(season, its files as `modis_items`) for each season, with GDAL set up to read them.

    One season at a time: a read token lasts about an hour."""
    os.environ.update(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif",
                      GDAL_HTTP_MAX_RETRY="5", GDAL_HTTP_RETRY_DELAY="2", VSI_CACHE="FALSE")
    for season in range(first_season, last_season + 1):
        yield season, modis_items(bounds, dt.date(season, 9, 1), dt.date(season + 1, 8, 31))


def bounded_map(pool, function, items, ahead):
    """`pool.map(function, items)` with at most `ahead` items submitted and not yet read: the readers of MODIS days
    outrun `SnowSeasons.observe`, and each day read holds its planes until it is observed."""
    pending = deque()
    for item in items:
        pending.append(pool.submit(function, item))
        if len(pending) == ahead:
            yield pending.popleft().result()
    while pending:
        yield pending.popleft().result()


def modis_planes(groups, bounds, first_season, last_season, canopy, workers=24):
    """Season planes on the MODIS grid around the bounds, from daily files fed as groups {date: [(platform, path)]}
    whose days increase from group to group. A pixel whose mean tree canopy cover in the Hansen GFC rasters
    `canopy` is more than DENSE_CANOPY_PERCENT is no data."""
    grid = modis_grid(bounds)
    state = SnowSeasons(first_season, last_season - first_season + 1, grid.shape)
    first, end, start = dt.date(first_season, 9, 1), 0, time.monotonic()
    with ThreadPoolExecutor(workers) as pool:
        for items in groups:
            days = sorted(items)
            read = lambda day: modis_day(items[day], grid)
            for day, (clear, snow) in zip(days, bounded_map(pool, read, days, 2 * workers)):
                end = (day - first).days + 1
                state.observe(end - 1, clear, snow)
            print(f"MODIS: {len(days)} days to {days[-1] if days else '-'}, {time.monotonic() - start:.0f} s",
                  file=sys.stderr, flush=True)
    planes = state.planes(end)
    planes[:, :, canopy_cover(grid, canopy) > DENSE_CANOPY_PERCENT] = NO_DATA
    return planes, grid


def copernicus_files(bounds):
    """{season: {layer: [GDAL path]}} of the Snow Phenology S2 products that touch the bounds, on CDSE S3."""
    west, south, east, north = bounds
    area = f"POLYGON(({west} {south},{east} {south},{east} {north},{west} {north},{west} {south}))"
    query = urllib.parse.urlencode({"$filter": f"contains(Name,'{COPERNICUS_PRODUCT}') and "
                                               f"OData.CSC.Intersects(area=geography'SRID=4326;{area}')", "$top": 1000})
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
    os.environ.update(AWS_ACCESS_KEY_ID=keys["CDSE_S3_ACCESS_KEY"], AWS_SECRET_ACCESS_KEY=keys["CDSE_S3_SECRET_KEY"],
                      AWS_S3_ENDPOINT="eodata.dataspace.copernicus.eu", AWS_VIRTUAL_HOSTING="FALSE", AWS_HTTPS="YES",
                      AWS_REGION="default", GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR")


def copernicus_planes(files, bounds, seasons):
    """Season planes on a 20 m LAEA grid around the bounds from Snow Phenology S2 rasters {season: {layer: [path]}}.

    A season of the range `seasons` without files is no data.
    """
    import rasterio
    from affine import Affine
    from rasterio.warp import Resampling, reproject, transform_bounds
    from rasterio.windows import Window, from_bounds, intersect

    left, bottom, right, top = transform_bounds("EPSG:4326", "EPSG:3035", *bounds, densify_pts=100)
    left, top = math.floor(left / 20) * 20 - 20, math.ceil(top / 20) * 20 + 20
    grid = Grid("EPSG:3035", Affine(20, 0, left, 0, -20, top),
                (math.ceil((top - bottom) / 20) + 1, math.ceil((right - left) / 20) + 1))
    extent = (left, top - 20 * grid.shape[0], left + 20 * grid.shape[1], top)
    planes = []
    for season in seasons:
        layers = {}
        for name in ("SCO", "SCM", "SCD"):
            # 65535 is no data and 420 inland water.
            layer = np.full(grid.shape, 65535, np.uint16)
            for path in files.get(season, {}).get(name, []):
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
        planes.append(encode(onset, length, season_start(season, season + 1), data))
    print(f"Read {sum(len(paths) for layers in files.values() for paths in layers.values())} files for {grid.shape} pixels",
          file=sys.stderr, flush=True)
    return np.stack(planes), grid


def subset(path, bounds, out):
    """Write the window of raster `path` that covers `bounds`, one pixel wider on each side, to `out`.

    The file keeps the name of the source file. A file whose raster misses the bounds writes nothing.
    """
    import rasterio
    from rasterio.warp import transform_bounds
    from rasterio.windows import Window, from_bounds

    target = out / Path(urllib.parse.urlsplit(path).path).name
    if target.exists():
        return
    with rasterio.open(path) as src:
        window = from_bounds(*transform_bounds("EPSG:4326", src.crs, *bounds, densify_pts=100), src.transform)
        # Rounded first: a bound on a pixel edge comes back a hair off it.
        col, row = round(window.col_off, 6), round(window.row_off, 6)
        left, top = math.floor(col) - 1, math.floor(row) - 1
        right, bottom = math.ceil(round(col + window.width, 6)) + 1, math.ceil(round(row + window.height, 6)) + 1
        left, top, right, bottom = max(left, 0), max(top, 0), min(right, src.width), min(bottom, src.height)
        if left >= right or top >= bottom:
            return
        window = Window(left, top, right - left, bottom - top)
        profile = {key: value for key, value in src.profile.items() if key not in ("blockxsize", "blockysize", "tiled")}
        profile.update(driver="GTiff", width=window.width, height=window.height, transform=src.window_transform(window),
                       compress="deflate")
        data = src.read(window=window)
    part = target.with_name(target.name + ".part")
    with rasterio.open(part, "w", **profile) as dst:
        dst.write(data)
    os.replace(part, target)


def fetch(source, bounds, first_season, last_season, out):
    """Write the window of every source file of the seasons to `out`, for `obc data fetch`."""
    out.mkdir(parents=True, exist_ok=True)
    if source == "copernicus-hr-wsi":
        cdse_credentials()
        files = copernicus_files(bounds)
        paths = {path for season in range(first_season, last_season + 1)
                 for layer in files.get(season, {}).values() for path in layer}
        with ThreadPoolExecutor(4) as pool:
            list(pool.map(lambda path: subset(path, bounds, out), paths))
        return
    for season, items in modis_seasons(bounds, first_season, last_season):
        # The search can list a file twice, and two writers of one file collide.
        hrefs = {urllib.parse.urlsplit(href).path: href for files in items.values() for _, href in files}
        with ThreadPoolExecutor(24) as pool:
            list(pool.map(lambda href: subset(href, bounds, out), hrefs.values()))
        print(f"Season {season}/{(season + 1) % 100:02d}: {len(hrefs)} files", file=sys.stderr, flush=True)


def step():
    """The `obc data` step `planner/snow`: `snow.pmtiles` from the HR-WSI Snow Phenology windows of the `hr-wsi`
    snapshot where they have data, and else from the MODIS daily windows of `modis-snow` with the tree canopy of
    `hansen-gfc`. A region outside HR-WSI has no `hr-wsi` snapshot."""
    request = step_request.read()
    options = request["options"]
    bounds, (first, last) = options["bounds"], options["seasons"]
    seasons = range(first, last + 1)
    days = {}
    for name, path in sorted(step_request.files(request, "modis-snow").items()):
        platform, year, day = re.fullmatch(r"(MOD|MYD)10A1\.A(\d{4})(\d{3})\..*\.tif", name).groups()
        days.setdefault(dt.date(int(year), 1, 1) + dt.timedelta(int(day) - 1), []).append((platform, path))
    canopy = [path for _, path in sorted(step_request.files(request, "hansen-gfc").items())]
    modis = modis_planes([days], bounds, first, last, canopy)
    sources = [("nasa-modis", lambda chunk: modis)]
    if "hr-wsi" in request["snapshots"]:
        files = {}
        for name, path in sorted(step_request.files(request, "hr-wsi").items()):
            season, layer = re.fullmatch(r".*_(\d{4})0901P1Y_.*_(SCO|SCM|SCD)\.tif", name).groups()
            files.setdefault(int(season), {}).setdefault(layer, []).append(str(path))
        sources.insert(0, ("copernicus-hr-wsi", lambda chunk: copernicus_planes(files, chunk, seasons)))
    count = bake(sources, first, len(seasons), bounds, Path(request["output"]) / "snow.pmtiles",
                 options["attribution"].format(year=options["year"]))
    step_request.metrics(request, {"tiles": count})


def main():
    # The step gets its credit and its versions in its options, so only this command line reads
    # the registry and the versions.
    from . import data_registry, planner_sources

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("region", nargs="?", help="region id: a box region in data/regions/ and the default output folder")
    parser.add_argument("--bounds", type=geo.bounds, help="west,south,east,north instead of the box of the region in data/regions/")
    parser.add_argument("--output", type=Path, help="default: ~/.cache/obc/planner/REGION/maps/snow.pmtiles")
    parser.add_argument("--source", choices=SOURCES, default="nasa-modis")
    parser.add_argument("--first-season", type=int, default=2000, help="the first NASA season")
    parser.add_argument("--last-season", type=int, default=2024, help="the last NASA season (2024 ends in June 2025)")
    parser.add_argument("--fetch", type=Path, help="only write the source windows of the seasons to this directory")
    parser.add_argument("--fetch-trails", type=Path, help="only write the Overpass answer of the trails to trails.json in this directory")
    args = parser.parse_args()
    if not args.region and not ((args.fetch or args.fetch_trails) and args.bounds):
        parser.error("give a region, or --bounds with --fetch or --fetch-trails")
    bounds = args.bounds or data_registry.region_box(args.region)
    if args.fetch_trails:
        args.fetch_trails.mkdir(parents=True, exist_ok=True)
        (args.fetch_trails / "trails.json").write_bytes(overpass_trails(bounds))
        return
    if args.fetch:
        fetch(args.source, bounds, args.first_season, args.last_season, args.fetch)
        return
    output = args.output or Path.home() / ".cache/obc/planner" / args.region / "maps/snow.pmtiles"
    start = time.monotonic()
    if args.source == "copernicus-hr-wsi":
        cdse_credentials()
        seasons = range(min(files := copernicus_files(bounds)), max(files) + 1)
        source = lambda chunk: copernicus_planes(copernicus_files(chunk), chunk, seasons)
        credit = data_registry.attribution("hr-wsi", year=dt.date.today().year)
    else:
        seasons = range(args.first_season, args.last_season + 1)
        canopy = [CANOPY.format(planner_sources.VERSIONS["hansen-gfc"], name) for name in canopy_tiles(bounds)]
        groups = (items for _, items in modis_seasons(bounds, args.first_season, args.last_season))
        modis = modis_planes(groups, bounds, args.first_season, args.last_season, canopy)
        source = lambda chunk: modis
        credit = f"{data_registry.attribution('modis-snow')}; tree canopy: {data_registry.attribution('hansen-gfc')}"
    count = bake([(args.source, source)], seasons.start, len(seasons), bounds, output, credit)
    print(json.dumps({"output": str(output), "bytes": output.stat().st_size, "tiles": count, "seasons": len(seasons),
                      "total_s": round(time.monotonic() - start)}))


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
