"""Bake the planner snow layer of one region: `snow.pmtiles`, as `specs/planner-snow-tiles.md` defines it.

    uv run --with-requirements tools/requirements-planner-snow.txt python -m tools.planner_snow REGION

The default source is NASA MODIS daily snow cover from the anonymous Microsoft Planetary Computer
copy. It ends in June 2025. The bake reads only the region window of each daily file and writes no
raw files. Seasons after June 2025 are not supported yet.

`--source copernicus-hr-wsi` reads the HR-WSI Snow Phenology S2 yearly rasters (20 m) instead. It
reads the window of one zoom-9 tile at a time from each file on the Copernicus Data Space S3 endpoint
`https://eodata.dataspace.copernicus.eu`, with `CDSE_S3_ACCESS_KEY` and `CDSE_S3_SECRET_KEY` in
`~/.config/openbikecomputer/cdse-s3.env`.
"""

import argparse
from collections import namedtuple
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
import warnings

import numpy as np

from . import planner_maps as maps

NO_SNOW, FULL, NO_DATA = 253, 254, 255
LAST_STEP = 182
TILE = 256
RECIPES = maps.ROOT / "tools/planner-regions"
# `canopy`: MODIS sees the canopy, not the snow under it. The Copernicus input corrects for trees.
# `smooth`: 20 m day values are noisy, so the lower zooms smooth them before they blend.
SOURCES = {
    "nasa-modis": {"resolution_m": 500, "canopy": True, "smooth": False,
                   "attribution": "NASA MODIS snow cover MOD10A1/MYD10A1 (NSIDC); tree canopy: Hansen/UMD/Google/USGS/NASA"},
    "copernicus-hr-wsi": {"resolution_m": 20, "canopy": False, "smooth": True,
                          "attribution": f"© European Union, Copernicus Land Monitoring Service {dt.date.today().year}, "
                                         "European Environment Agency (EEA): HR-WSI Snow Phenology"},
}
CANOPY = "https://storage.googleapis.com/earthenginepartners-hansen/GFC-2023-v1.11/Hansen_GFC-2023-v1.11_treecover2000_{}.tif"
# The owner chose 75 % canopy cover as "dense": below it, MODIS still sees the snow between the trees.
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
        with np.errstate(all="ignore"), warnings.catch_warnings():
            warnings.simplefilter("ignore", RuntimeWarning)  # a pixel without dated inputs has no median
            centre = np.nan_to_num(np.nanmedian(np.where(dated_mask[:, :, None], values, np.nan).astype(np.float32), axis=0))
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


def tile_bounds(z, x, y):
    n = 2 ** z
    lat = lambda row: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * row / n))))
    return x / n * 360 - 180, lat(y + 1), (x + 1) / n * 360 - 180, lat(y)


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
    """Names of the 10° Hansen tiles, named by their north-west corner, that the bounds touch."""
    west, south, east, north = bounds
    return [f"{abs(lat):02d}{'N' if lat >= 0 else 'S'}_{abs(lon):03d}{'E' if lon >= 0 else 'W'}"
            for lat in range(math.ceil(south / 10) * 10, math.ceil(north / 10) * 10 + 1, 10)
            for lon in range(math.floor(west / 10) * 10, math.ceil(east / 10) * 10, 10)]


def canopy_cover(z, x, y):
    """Mean tree canopy cover in percent of each pixel of tile z/x/y, in the year 2000."""
    import rasterio
    from rasterio.transform import from_bounds
    from rasterio.warp import Resampling, reproject, transform_bounds
    from rasterio.windows import Window, from_bounds as window_from_bounds

    bounds = tile_bounds(z, x, y)
    cover = np.zeros((TILE, TILE), np.float32)
    target = from_bounds(*transform_bounds("EPSG:4326", "EPSG:3857", *bounds), TILE, TILE)
    for name in canopy_tiles(bounds):
        with rasterio.open(CANOPY.format(name)) as src:
            window = window_from_bounds(*bounds, src.transform).intersection(Window(0, 0, src.width, src.height))
            window = window.round_offsets().round_lengths()
            reproject(src.read(1, window=window).astype(np.float32), cover, src_transform=src.window_transform(window),
                      src_crs=src.crs, dst_transform=target, dst_crs="EPSG:3857", resampling=Resampling.average,
                      init_dest_nodata=False)
    return cover


def trails(bounds):
    """OSM `highway=path|track` segments inside the bounds: midpoint longitude, latitude and length in metres."""
    west, south, east, north = bounds
    query = f'[out:json][timeout:180];way["highway"~"^(path|track)$"]({south},{west},{north},{east});out geom;'
    request = urllib.request.Request(OVERPASS, data=urllib.parse.urlencode({"data": query}).encode(),
                                     headers={"User-Agent": USER_AGENT})
    for attempt in range(4):
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                ways = json.load(response)["elements"]
            break
        except urllib.error.HTTPError as error:
            # Overpass answers 429 and 504 when it is busy.
            if error.code not in (429, 504) or attempt == 3:
                raise
            time.sleep(60 * (attempt + 1))
    lon, lat, metres = [], [], []
    for way in ways:
        points = np.radians([[p["lon"], p["lat"]] for p in way.get("geometry", [])])
        if len(points) < 2:
            continue
        a, b = points[:-1], points[1:]
        h = np.sin((b[:, 1] - a[:, 1]) / 2) ** 2 + np.cos(a[:, 1]) * np.cos(b[:, 1]) * np.sin((b[:, 0] - a[:, 0]) / 2) ** 2
        metres.append(2 * 6371008.8 * np.arcsin(np.sqrt(h)))
        mid = np.degrees((a + b) / 2)
        lon.append(mid[:, 0])
        lat.append(mid[:, 1])
    lon, lat, metres = (np.concatenate(v) for v in (lon, lat, metres))
    inside = (lon >= west) & (lon <= east) & (lat >= south) & (lat <= north)
    return lon[inside], lat[inside], metres[inside]


def bake(source, first_season, seasons, name, bounds, output, trail_segments=None):
    """Write the archive; return the tile count and, with trail segments, the share of trail length on no data.

    `source(bounds)` gives the season planes and their grid around the bounds of one chunk tile.
    """
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    top = max_zoom(SOURCES[name]["resolution_m"], bounds)
    chunk = min(CHUNK_ZOOM, top)
    west, south, east, north = bounds
    tiles, masked, planes, grid = {}, 0.0, None, None
    if trail_segments is not None:
        lon, lat, metres = trail_segments
        n = 2 ** top * TILE
        px = ((lon + 180) / 360 * n).astype(np.int64)
        py = ((1 - np.log(np.tan(np.radians(lat)) + 1 / np.cos(np.radians(lat))) / np.pi) / 2 * n).astype(np.int64)

    def build(z, x, y):
        nonlocal masked, planes, grid
        w, s, e, n_ = tile_bounds(z, x, y)
        if w >= east or e <= west or s >= north or n_ <= south:
            return None
        if z == chunk:
            planes, grid = source((max(w, west), max(s, south), min(e, east), min(n_, north)))
        if z == top:
            body = sample(planes, grid, z, x, y)
            lon_c, lat_c = tile_lonlat(z, x, y)
            outside = ((lon_c < west) | (lon_c > east) | (lat_c < south) | (lat_c > north)).reshape(TILE, TILE)
            body[:, :, outside] = NO_DATA
            if SOURCES[name]["canopy"]:
                body[:, :, canopy_cover(z, x, y) > DENSE_CANOPY_PERCENT] = NO_DATA
            if trail_segments is not None:
                here = (px // TILE == x) & (py // TILE == y)
                missing = (body[:, 0] == NO_DATA).all(0)
                masked += metres[here][missing[py[here] % TILE, px[here] % TILE]].sum()
        else:
            children = {(dx, dy): build(z + 1, 2 * x + dx, 2 * y + dy) for dx in (0, 1) for dy in (0, 1)}
            body = parent(children, seasons, SOURCES[name]["smooth"])
        if z == chunk:
            planes = grid = None
        if (body != NO_DATA).any():
            tiles[zxy_to_tileid(z, x, y)] = gzip.compress(body.tobytes(), mtime=0)
        return body

    build(0, 0, 0)
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
                "resolution_m": meta["resolution_m"], "attribution": meta["attribution"],
            })
            stream.flush()
            os.replace(stream.name, output)
        except BaseException:
            os.unlink(stream.name)
            raise
    share = None if trail_segments is None else masked / max(trail_segments[2].sum(), 1e-9)
    return len(tiles), share


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
    return Grid(SINUSOIDAL, Affine(size, 0, MODIS_X0 + c0 * size, 0, -size, MODIS_Y0 - r0 * size), (r1 - r0, c1 - c0)), (r0, c0)


def modis_items(bounds, start, end):
    """{date: [(platform, h, v, href)]} of the MOD10A1/MYD10A1 files from `start` to `end`."""
    body = {"collections": ["modis-10A1-061"], "bbox": list(bounds), "datetime": f"{start}/{end}", "limit": 1000}
    items, page = {}, http_json(STAC, body)
    while True:
        for feature in page["features"]:
            p = feature["properties"]
            items.setdefault(dt.date.fromisoformat(p["start_datetime"][:10]), []).append(
                (feature["id"][:3], p["modis:horizontal-tile"], p["modis:vertical-tile"], feature["assets"]["NDSI_Snow_Cover"]["href"]))
        link = next((link for link in page.get("links", []) if link["rel"] == "next"), None)
        if link is None:
            break
        page = http_json(link["href"], link.get("body")) if link.get("method") == "POST" else http_json(link["href"])
    # Each file needs a read token of its storage account and container; a token is valid for about an hour.
    container = lambda href: (urllib.parse.urlsplit(href).netloc.split(".")[0], urllib.parse.urlsplit(href).path.split("/")[1])
    keys = {container(file[3]) for files in items.values() for file in files}
    tokens = {key: http_json(SAS.format(*key))["token"] for key in keys}
    sign = lambda href: f"{href}?{tokens[container(href)]}"
    return {day: [(*rest, sign(href)) for *rest, href in files] for day, files in items.items()}


def modis_day(files, grid, origin):
    """Clear and snow masks of one day: Terra first, Aqua where Terra is not clear."""
    import rasterio
    from rasterio.windows import Window

    rows, cols = grid.shape
    r0, c0 = origin
    ndsi = {"MOD": np.full(grid.shape, NO_DATA, np.uint8), "MYD": np.full(grid.shape, NO_DATA, np.uint8)}
    for platform, h, v, href in files:
        top, bottom = max(r0, v * MODIS_PIXELS), min(r0 + rows, (v + 1) * MODIS_PIXELS)
        left, right = max(c0, h * MODIS_PIXELS), min(c0 + cols, (h + 1) * MODIS_PIXELS)
        if top >= bottom or left >= right:
            continue
        for attempt in range(5):
            try:
                with rasterio.open(href) as src:
                    if abs(src.transform.c - (MODIS_X0 + h * MODIS_TILE_M)) > 1 or abs(src.transform.f - (MODIS_Y0 - v * MODIS_TILE_M)) > 1:
                        raise ValueError(f"{href} is not on the MODIS sinusoidal grid")
                    window = Window(left - h * MODIS_PIXELS, top - v * MODIS_PIXELS, right - left, bottom - top)
                    ndsi[platform][top - r0:bottom - r0, left - c0:right - c0] = src.read(1, window=window)
                break
            except rasterio.errors.RasterioIOError:
                if attempt == 4:
                    raise
                time.sleep(3 * (attempt + 1))
    # 0–100 is NDSI × 100 and 254 a saturated detector; the other values are cloud, night, water or fill.
    clear = lambda value: (value <= 100) | (value == 254)
    value = np.where(clear(ndsi["MOD"]), ndsi["MOD"], ndsi["MYD"])
    return clear(value), clear(value) & (value >= MODIS_SNOW_NDSI)


def nasa_planes(bounds, first_season, last_season, workers=24):
    """Season planes on the MODIS grid, streamed one season of daily files at a time."""
    os.environ.update(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif",
                      GDAL_HTTP_MAX_RETRY="5", GDAL_HTTP_RETRY_DELAY="2", VSI_CACHE="FALSE")
    grid, origin = modis_grid(bounds)
    state = SnowSeasons(first_season, last_season - first_season + 1, grid.shape)
    end, start = 0, time.monotonic()
    with ThreadPoolExecutor(workers) as pool:
        for season in range(first_season, last_season + 1):
            first, last = dt.date(season, 9, 1), dt.date(season + 1, 8, 31)
            items = modis_items(bounds, first, last)
            days = sorted(items)
            for day, (clear, snow) in zip(days, pool.map(lambda d: modis_day(items[d], grid, origin), days)):
                index = season_start(first_season, season) + (day - first).days
                state.observe(index, clear, snow)
                end = index + 1
            print(f"Season {season}/{(season + 1) % 100:02d}: {len(days)} days, {time.monotonic() - start:.0f} s",
                  file=sys.stderr, flush=True)
    return state.planes(end), grid


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


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("region", help="region name: the recipe in tools/planner-regions and the default output folder")
    parser.add_argument("--bounds", type=maps.bounds, help="west,south,east,north instead of the recipe bounds")
    parser.add_argument("--output", type=Path, help="default: ~/.cache/obc/planner/REGION/maps/snow.pmtiles")
    parser.add_argument("--source", choices=SOURCES, default="nasa-modis")
    parser.add_argument("--first-season", type=int, default=2000, help="the first NASA season")
    parser.add_argument("--last-season", type=int, default=2024, help="the last NASA season (2024 ends in June 2025)")
    parser.add_argument("--trails", action="store_true", help="report the share of OSM path and track length with no data in every season")
    args = parser.parse_args()
    bounds = args.bounds or json.loads((RECIPES / f"{args.region}.json").read_text())["bounds"]
    output = args.output or Path.home() / ".cache/obc/planner" / args.region / "maps/snow.pmtiles"
    start = time.monotonic()
    if args.source == "copernicus-hr-wsi":
        cdse_credentials()
        seasons = range(min(files := copernicus_files(bounds)), max(files) + 1)
        source = lambda chunk: copernicus_planes(copernicus_files(chunk), chunk, seasons)
    else:
        planes, grid = nasa_planes(bounds, args.first_season, args.last_season)
        seasons = range(args.first_season, args.first_season + planes.shape[0])
        source = lambda chunk: (planes, grid)
    count, share = bake(source, seasons.start, len(seasons), args.source, bounds, output, trails(bounds) if args.trails else None)
    report = {"output": str(output), "bytes": output.stat().st_size, "tiles": count, "seasons": len(seasons),
              "total_s": round(time.monotonic() - start)}
    if share is not None:
        report["no_data_trail_share"] = round(share, 3)
    print(json.dumps(report))


if __name__ == "__main__":
    main()
