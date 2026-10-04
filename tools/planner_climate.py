"""Bake the planner climate layer of one region: `climate.pmtiles`, as `specs/planner-climate-tiles.md` defines it.

    uv run --with-requirements tools/requirements-planner-climate.txt python -m tools.planner_climate REGION

The source is ERA5-Land, read from the ECMWF ARCO geo-chunked Zarr stores with the CDS personal access
token in `~/.cdsapirc`. The bake reads only the source chunks around the region and keeps them in
`~/.cache/obc/planner/sources/era5-land`. A cached chunk serves every later bake that it holds final
data for, so an interrupted bake or a yearly update downloads only what is missing.
`--check` bakes again from the cache only and fails unless the result equals the archive byte for byte.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
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
import urllib.request

import numpy as np

from . import planner_maps as maps

YEARS = 10
WEEKS = 52
# ERA5-Land has too many light-rain days. The owner chose a wet day as a day with at least this much
# ERA5-Land rain: the threshold whose wet-day frequency matches the days with at least 1 mm at the
# 58 DWD stations in Baden-Württemberg that cover the ten years (DWD Climate Data Center, daily KL).
WET_MM = 2.3
# ERA5-Land weekly rain totals are too high, most in winter. The bake multiplies them by the factor of the
# week's month, January first: the sum of the station weekly totals ÷ the sum of the ERA5-Land weekly totals
# of their cells, at the same 58 DWD stations over the ten years.
RAIN_FACTORS = (0.791, 0.766, 0.759, 0.755, 0.883, 0.865, 0.942, 0.882, 0.869, 0.875, 0.866, 0.744)
# ERA5-Land daytime wind is too low. The bake multiplies the weekly daytime wind by this factor: the sum of the station
# weekly daytime means ÷ the sum of the ERA5-Land weekly means of their cells, over the ten years at the 27 DWD stations in
# Baden-Württemberg with hourly wind that stand within 150 m of the height of their cell (DWD Climate Data Center, hourly wind).
WIND_FACTOR = 1.315
SECTORS = 16
# Final ERA5-Land replaces the preliminary ERA5-Land-T data about two months after each month (ECMWF,
# "ERA5-Land: data documentation"); DKRZ reports up to three months. The ARCO stores have no `expver`
# to tell them apart. So a source hour counts as final only in a store that reaches this many days past it.
FINAL_AFTER_DAYS = 120
GRID_COLS, GRID_ROWS = 3600, 1801  # the native 0.1° ERA5-Land grid
EPOCH = dt.datetime(1950, 1, 2)  # hour 0 of the Zarr time axis
TIME_CHUNK, CHUNK_ROWS, CHUNK_COLS = 33792, 4, 8  # the Zarr chunk shape
MARGIN_HOURS = 48  # source hours before and after the years: a local day reaches up to 12 h past its UTC day
LAPSE_RADIUS = 2  # cells on each side of a cell in its lapse-rate regression
# Below this relief the cells around a cell do not separate height from distance. The correction on
# such flat ground is small, so it uses the standard atmosphere instead.
LAPSE_MIN_RELIEF_M = 100
STANDARD_LAPSE = -6.5
ARCO = "https://arco.datastores.ecmwf.int/cadl-arco-geo-{}/arco/reanalysis_era5_land/{}/geoChunked.zarr"
# The hourly source variables: the ARCO store group and its bucket.
SOURCE = {"t2m": ("sfc-2m-temperature", "007"), "u10": ("sfc-wind", "008"), "v10": ("sfc-wind", "008"),
          "tp": ("sfc-pressure-precipitation", "009")}
OROGRAPHY = ("https://confluence.ecmwf.int/download/attachments/140385202/geo_1279l4_0.1x0.1.grib2_v4_unpack.nc",
             "6fe9d064e7eae98bfe20348430bc4290bc94daa838b560c355999cd85cb1a559")
GRAVITY = 9.80665
DOI = "10.24381/cds.e2161bac"
CACHE = Path.home() / ".cache/obc/planner/sources/era5-land"
RECIPES = maps.ROOT / "tools/planner-regions"
USER_AGENT = "OpenBikeComputer planner climate bake (https://github.com/timohueser/OpenBikeComputer)"

# Hourly inputs in display units.
HOURLY = {
    "t2m": lambda source: source["t2m"] - 273.15,
    "tp": lambda source: np.maximum(source["tp"], 0) * 1000,  # de-accumulation leaves tiny negative values
    "speed": lambda source: np.hypot(source["u10"], source["v10"]),
}
# Daily values of a local solar day: the hourly input, the local hour stamps and the reducer. An
# accumulated value at stamp h covers the hour before h, so the rain of a day has stamps 1 to 24.
DAILY = {
    "tmax": ("t2m", range(0, 24), np.max),
    "tmin": ("t2m", range(0, 24), np.min),
    "rain": ("tp", range(1, 25), np.sum),
    "wind": ("speed", range(9, 19), np.mean),
}
DAYTIME = DAILY["wind"][1]
# Stored weekly fields: the daily value and the reducer over the days of the week.
WEEKLY = {
    "wet_days": ("rain", lambda days: (days >= WET_MM).sum(0)),
    "rain": ("rain", lambda days: days.sum(0)),
    "tmax": ("tmax", lambda days: days.mean(0)),
    "tmin": ("tmin", lambda days: days.mean(0)),
    "wind": ("wind", lambda days: days.mean(0)),
}
# Integer codes: type, missing code, highest code and segments (first code, its value, value step).
# A value takes the nearest code of its segment and is clamped to the code range.
CODES = {
    "wet_days": ("u1", 255, 254, [(0, 0, 1)]),
    "wet_share": ("u1", 255, 100, [(0, 0, 1)]),
    "rain": ("u1", 255, 254, [(0, 0, 1), (100, 100, 5)]),
    "tmax": ("i1", -128, 127, [(-127, -63.5, 0.5)]),
    "tmin": ("i1", -128, 127, [(-127, -63.5, 0.5)]),
    "wind": ("u1", 255, 254, [(0, 0, 0.5)]),
    "orography": ("<i2", -32768, 32767, [(-32767, -32767, 1)]),
    "lapse_tmax": ("i1", -128, 127, [(-127, -12.7, 0.1)]),
    "lapse_tmin": ("i1", -128, 127, [(-127, -12.7, 0.1)]),
    "rose": ("u1", 255, 200, [(0, 0, 0.5)]),
}
# Tile levels: zoom, then cell columns and rows of a tile and its planes (name, values per cell). Both
# levels carry the terrain correction, so a route or a chart needs no overview tile.
OVERVIEW, DETAIL = 8, 9
LEVELS = {
    OVERVIEW: (24, 16, [("orography", 1), ("lapse_tmax", 12), ("lapse_tmin", 12), ("rose", 12 * SECTORS), ("wet_share", WEEKS),
                        ("rain", WEEKS), ("tmax", WEEKS), ("tmin", WEEKS), ("wind", WEEKS)]),
    DETAIL: (12, 8, [("orography", 1), ("lapse_tmax", 12), ("lapse_tmin", 12), *((name, YEARS * WEEKS) for name in WEEKLY)]),
}


def encode(name, values):
    kind, missing, top, segments = CODES[name]
    values = np.asarray(values, np.float64)
    index = np.clip(np.searchsorted([value for _, value, _ in segments], values, "right") - 1, 0, len(segments) - 1)
    first, base, step = (np.array([segment[k] for segment in segments])[index] for k in range(3))
    codes = np.clip(first + np.floor((values - base) / step + 0.5), segments[0][0], top)
    return np.where(np.isnan(values), missing, np.nan_to_num(codes)).astype(kind)


def decode(name, codes):
    _, missing, _, segments = CODES[name]
    codes = np.asarray(codes, np.float64)
    index = np.clip(np.searchsorted([code for code, _, _ in segments], codes, "right") - 1, 0, len(segments) - 1)
    first, base, step = (np.array([segment[k] for segment in segments], np.float64)[index] for k in range(3))
    return np.where(codes == missing, np.nan, base + (codes - first) * step)


def solar_offset(lon):
    """Local solar time minus UTC in whole hours."""
    return np.floor(np.asarray(lon) / 15 + 0.5).astype(int)


def calendar(first_year):
    """Month (1–12) and week of each day of the years; the week counts from 0 in the first year.

    Week w of a year holds its days 7w to 7w + 6; the last week also takes days 365 and 366.
    """
    start = dt.date(first_year, 1, 1)
    days = [start + dt.timedelta(d) for d in range((dt.date(first_year + YEARS, 1, 1) - start).days)]
    month = np.array([day.month for day in days])
    week = np.array([(day.year - first_year) * WEEKS + min((day.timetuple().tm_yday - 1) // 7, WEEKS - 1) for day in days])
    return month, week


def week_month(week):
    """Month (1–12) of week slot `week`: the month of its day 7 week + 3 in a year that is not a leap year."""
    return (dt.date(2001, 1, 1) + dt.timedelta(7 * week + 3)).month


def local_days(values, offset, stamps, days):
    """Samples (days, stamps, cells) of local solar days; `values` (hours, cells) start MARGIN_HOURS before day 0 in UTC."""
    index = MARGIN_HOURS + 24 * np.arange(days)[:, None] + np.asarray(stamps)[None, :] - offset
    return values[index]


def aggregate(source, lon, first_year):
    """Weekly fields {name: (years, weeks, cells)} with calibrated rain and wind, the monthly means {tmax, tmin: (12, cells)} and the
    daytime wind rose (12, sectors, cells) in percent, from hourly source values {variable: (hours, cells)}."""
    month, week = calendar(first_year)
    hourly = {name: rule(source) for name, rule in HOURLY.items()}
    offsets, days, cells = solar_offset(lon), len(month), len(lon)
    daily = {name: np.full((days, cells), np.nan) for name in DAILY}
    u = np.full((days, len(DAYTIME), cells), np.nan)
    v = np.full_like(u, np.nan)
    for offset in np.unique(offsets):
        here = offsets == offset
        for name, (variable, stamps, reducer) in DAILY.items():
            daily[name][:, here] = reducer(local_days(hourly[variable][:, here], offset, stamps, days), axis=1)
        u[:, :, here] = local_days(source["u10"][:, here], offset, DAYTIME, days)
        v[:, :, here] = local_days(source["v10"][:, here], offset, DAYTIME, days)
    edges = np.flatnonzero(np.diff(week, prepend=-1, append=YEARS * WEEKS))
    weekly = {}
    for name, (value, reducer) in WEEKLY.items():
        values = daily[value]
        weeks = [np.where(np.isnan(values[a:b]).any(0), np.nan, reducer(values[a:b])) for a, b in zip(edges[:-1], edges[1:])]
        weekly[name] = np.stack(weeks).reshape(YEARS, WEEKS, cells)
    weekly["rain"] *= np.array([RAIN_FACTORS[week_month(w) - 1] for w in range(WEEKS)])[None, :, None]
    weekly["wind"] *= WIND_FACTOR
    monthly = {name: np.stack([daily[name][month == m].mean(0) for m in range(1, 13)]) for name in ("tmax", "tmin")}
    # The direction the wind comes from, clockwise from north; sector 0 is centred on north.
    sector = np.floor(np.degrees(np.arctan2(-u, -v)) % 360 / (360 / SECTORS) + 0.5).astype(int) % SECTORS
    counts = np.stack([np.stack([((sector == s) & (month == m)[:, None, None]).sum((0, 1)) for s in range(SECTORS)])
                       for m in range(1, 13)]).astype(np.float64)
    rose = 100 * counts / np.maximum(counts.sum(1, keepdims=True), 1)
    rose[:, :, np.isnan(u).any((0, 1))] = np.nan
    return weekly, monthly, rose


def lapse_rates(mean, height, lat, lon, radius=LAPSE_RADIUS):
    """Lapse rate in K/km (12, rows, cols) of each cell at least `radius` from the edge of the grid.

    A cell takes the height coefficient of a least-squares plane T = a + b·height + c·lon + d·lat through
    the cells within `radius`. With less relief than LAPSE_MIN_RELIEF_M, it takes STANDARD_LAPSE.
    """
    months, rows, cols = mean.shape
    x, y = np.broadcast_to(lon, (rows, cols)), np.broadcast_to(np.asarray(lat)[:, None], (rows, cols))
    out = np.full(mean.shape, np.nan)
    for r in range(radius, rows - radius):
        for c in range(radius, cols - radius):
            window = np.s_[r - radius:r + radius + 1, c - radius:c + radius + 1]
            t = mean[(slice(None), *window)].reshape(months, -1)
            ok = ~np.isnan(t).any(0) & ~np.isnan(height[window].ravel())
            if np.isnan(mean[:, r, c]).any():
                continue
            if ok.sum() < 6 or np.ptp(height[window].ravel()[ok]) < LAPSE_MIN_RELIEF_M:
                out[:, r, c] = STANDARD_LAPSE
                continue
            design = np.stack([np.ones(ok.sum()), height[window].ravel()[ok] / 1000, x[window].ravel()[ok], y[window].ravel()[ok]], 1)
            out[:, r, c] = np.linalg.lstsq(design, t[:, ok].T, rcond=None)[0][1]
    return out


class Region:
    """The grid cells that touch the bounds, padded by `pad` cells.

    Rows count from the north (row 0 at 90° N) and columns from 180° W, as the spec's cell index does.
    """

    def __init__(self, bounds, pad=LAPSE_RADIUS):
        west, south, east, north = bounds
        edge = lambda value: round(value * 10, 6)
        self.rows = np.arange(math.floor(edge(90 - north) - 0.5) + 1 - pad, math.ceil(edge(90 - south) + 0.5) + pad)
        self.cols = np.arange(math.floor(edge(west + 180) - 0.5) + 1 - pad, math.ceil(edge(east + 180) + 0.5) + pad)
        self.pad = pad
        self.lat = 90 - self.rows / 10
        self.lon = self.cols / 10 - 180

    def zarr_index(self):
        """Row and column of each cell in the Zarr stores, which start at 90° S and 179.9° W."""
        return GRID_ROWS - 1 - self.rows, (self.cols - 1) % GRID_COLS


def chunk_plan(region, first_year):
    """The source hours [start, end) of the bake, the Zarr time chunks and the (row, col) chunks."""
    start = hour(dt.date(first_year, 1, 1)) - MARGIN_HOURS
    end = hour(dt.date(first_year + YEARS, 1, 1)) + MARGIN_HOURS
    rows, cols = region.zarr_index()
    spatial = sorted({(int(r) // CHUNK_ROWS, int(c) // CHUNK_COLS) for r in rows for c in cols})
    return start, end, range(start // TIME_CHUNK, (end - 1) // TIME_CHUNK + 1), spatial


def token():
    match = re.search(r"^key:\s*(\S+)", (Path.home() / ".cdsapirc").read_text(), re.M)
    if not match:
        raise ValueError("Add the CDS personal access token to ~/.cdsapirc as `key: TOKEN`")
    return match[1]


def fetch(url, key=None):
    headers = {"User-Agent": USER_AGENT, **({"Authorization": f"Bearer {key}"} if key else {})}
    for attempt in range(5):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=300) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code < 500 and error.code != 429 or attempt == 4:
                raise
        except (urllib.error.URLError, TimeoutError):
            if attempt == 4:
                raise
        time.sleep(5 * (attempt + 1))


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def blosc(data, dtype):
    from numcodecs import Blosc
    return np.frombuffer(Blosc().decode(data), dtype)


def hour(day):
    """Hour of 00 UTC on `day` on the Zarr time axis."""
    return (dt.datetime.combine(day, dt.time()) - EPOCH) // dt.timedelta(hours=1)


def stores(key):
    """{variable: (URL, hour count)}, after a check of each store's layout against the constants here."""
    result = {}
    for variable, (group, bucket) in SOURCE.items():
        base = ARCO.format(bucket, group)
        meta = json.loads(fetch(base + "/.zmetadata", key))["metadata"]
        array, axis = meta[f"{variable}/.zarray"], meta["time/.zarray"]
        if (array["dtype"] != "<f4" or array["shape"][1:] != [GRID_ROWS, GRID_COLS] or array["order"] != "C"
                or array["chunks"] != [TIME_CHUNK, CHUNK_ROWS, CHUNK_COLS] or array["filters"]
                or array["compressor"]["id"] != "blosc" or meta["time/.zattrs"]["units"] != "hours since 1970-01-01"):
            raise ValueError(f"Unexpected ERA5-Land store layout: {variable}")
        hours, step = array["shape"][0], axis["chunks"][0]
        lat, lon = (blosc(fetch(f"{base}/{name}/0", key), "<f8") for name in ("latitude", "longitude"))
        if not (np.allclose(lat, np.arange(GRID_ROWS) / 10 - 90) and np.allclose(lon, np.arange(1, GRID_COLS + 1) / 10 - 180)):
            raise ValueError(f"Unexpected ERA5-Land grid: {variable}")
        first = blosc(fetch(f"{base}/time/0", key), "<i8")[0]
        last = blosc(fetch(f"{base}/time/{(hours - 1) // step}", key), "<i8")[(hours - 1) % step]
        if first != (EPOCH - dt.datetime(1970, 1, 1)) // dt.timedelta(hours=1) or last != first + hours - 1:
            raise ValueError(f"ERA5-Land time axis is not hourly from 2 January 1950: {variable}")
        result[variable] = base, hours
    return result


class Source:
    """Source chunks from the cache, downloaded when missing.

    A cached chunk is named `T.Y.X.HOURS.SHA256`: its Zarr key, the store's hour count when it was
    downloaded, and the SHA-256 of its compressed bytes. It serves a bake when that hour count reaches
    its threshold, so every hour it holds for the bake was final data. An empty file is a chunk that the
    store omits because it has no data.
    """

    def __init__(self, final_hour, key=None, cache=CACHE):
        self.final_hour, self.key, self.cache = final_hour, key, cache
        self.layout = None
        self.digests = {variable: {} for variable in SOURCE}
        self.downloaded = 0

    def threshold(self, name):
        """The store hour count from which chunk `name` is final: FINAL_AFTER_DAYS past its end or past the bake."""
        return min(self.final_hour, (int(name.split(".")[0]) + 1) * TIME_CHUNK + FINAL_AFTER_DAYS * 24)

    def path(self, variable, name):
        threshold = self.threshold(name)
        found = [path for path in (self.cache / variable).glob(f"{name}.*") if int(path.name.split(".")[3]) >= threshold]
        if found:
            return max(found, key=lambda path: int(path.name.split(".")[3]))
        if self.key is None:
            raise ValueError(f"Source chunk {variable}/{name} is not in the cache")
        self.layout = self.layout or stores(self.key)
        base, hours = self.layout[variable]
        if hours < threshold:
            raise ValueError("ERA5-Land has no final data for the last year yet")
        try:
            data = fetch(f"{base}/{variable}/{name}", self.key)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            data = b""
        path = self.cache / variable / f"{name}.{hours}.{hashlib.sha256(data).hexdigest()}"
        path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".chunk-", delete=False) as stream:
            stream.write(data)
        os.replace(stream.name, path)
        for older in (self.cache / variable).glob(f"{name}.*"):
            if older != path:
                older.unlink()
        self.downloaded += len(data)
        return path

    def read(self, variable, name):
        path = self.path(variable, name)
        data = path.read_bytes()
        checksum = hashlib.sha256(data).hexdigest()
        if checksum != path.name.split(".")[4]:
            raise ValueError(f"Cached source chunk is damaged: {path}")
        self.digests[variable][name] = checksum
        shape = (TIME_CHUNK, CHUNK_ROWS, CHUNK_COLS)
        return blosc(data, "<f4").reshape(shape) if data else np.full(shape, np.nan, np.float32)

    def fingerprint(self):
        """{variable: SHA-256 of its sorted `KEY SHA256` lines}: the source chunks of the bake."""
        return {variable: hashlib.sha256("".join(f"{name} {checksum}\n" for name, checksum in sorted(chunks.items())).encode()).hexdigest()
                for variable, chunks in self.digests.items()}


def orography(region, key=None):
    """ERA5-Land orography in metres (rows, cols)."""
    import h5py

    url, checksum = OROGRAPHY
    path = CACHE / Path(url).name
    if not path.exists():
        if key is None:
            raise ValueError("ERA5-Land orography is not in the cache")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(fetch(url))
    if digest(path) != checksum:
        path.unlink()
        raise ValueError("ERA5-Land orography does not match its pinned checksum")
    with h5py.File(path) as file:
        z = file["z"][0]
    # This file starts at 90° N and 0° E.
    return z[region.rows][:, (region.cols + GRID_COLS // 2) % GRID_COLS] / GRAVITY


def climate(region, first_year, source, workers=8):
    """Weekly fields, monthly mean temperatures and the wind rose over the padded region grid."""
    start, end, times, spatial = chunk_plan(region, first_year)
    names = [(variable, f"{t}.{y}.{x}") for y, x in spatial for variable in SOURCE for t in times]
    with ThreadPoolExecutor(workers) as pool:
        list(pool.map(lambda name: source.path(*name), names))
    shape = (len(region.rows), len(region.cols))
    weekly = {name: np.full((YEARS, WEEKS, *shape), np.nan) for name in WEEKLY}
    monthly = {name: np.full((12, *shape), np.nan) for name in ("tmax", "tmin")}
    rose = np.full((12, SECTORS, *shape), np.nan)
    zarr_rows, zarr_cols = region.zarr_index()
    for y, x in spatial:
        rows = np.flatnonzero(zarr_rows // CHUNK_ROWS == y)
        cols = np.flatnonzero(zarr_cols // CHUNK_COLS == x)
        values = {}
        for variable in SOURCE:
            series = np.concatenate([source.read(variable, f"{t}.{y}.{x}") for t in times])
            series = series[start - times[0] * TIME_CHUNK:end - times[0] * TIME_CHUNK]
            cells = series[:, zarr_rows[rows] % CHUNK_ROWS][:, :, zarr_cols[cols] % CHUNK_COLS]
            values[variable] = cells.reshape(len(series), -1).astype(np.float64)
        lon = np.broadcast_to(region.lon[cols], (len(rows), len(cols))).ravel()
        w, m, r = aggregate(values, lon, first_year)
        grid = np.ix_(rows, cols)
        for name in weekly:
            weekly[name][(slice(None), slice(None), *grid)] = w[name].reshape(YEARS, WEEKS, len(rows), len(cols))
        for name in monthly:
            monthly[name][(slice(None), *grid)] = m[name].reshape(12, len(rows), len(cols))
        rose[(slice(None), slice(None), *grid)] = r.reshape(12, SECTORS, len(rows), len(cols))
    return weekly, monthly, rose


def planes(region, first_year, weekly, monthly, rose, height):
    """Values of each tile plane on the unpadded grid: {zoom: {name: (values per cell, rows, cols)}}.

    The overview means come from the quantised detail values, so a client that averages the detail
    rows of a cell gets the overview value.
    """
    core = np.s_[..., region.pad:len(region.rows) - region.pad, region.pad:len(region.cols) - region.pad]
    shape = height[core].shape
    detail = {name: decode(name, encode(name, weekly[name][core])) for name in WEEKLY}
    _, week = calendar(first_year)
    days = np.bincount(week).reshape(YEARS, WEEKS, 1, 1)
    def mean(values, weights=1):
        valid = ~np.isnan(values)
        with np.errstate(invalid="ignore", divide="ignore"):
            return np.where(valid.any(0), np.nansum(values, 0) / (valid * weights).sum(0), np.nan)
    terrain = {"orography": height[core][None],
               **{f"lapse_{name}": lapse_rates(monthly[name], height, region.lat, region.lon)[core] for name in monthly}}
    overview = {**terrain, "rose": rose[core].reshape(12 * SECTORS, *shape), "wet_share": 100 * mean(detail["wet_days"], days),
                **{name: mean(detail[name]) for name in ("rain", "tmax", "tmin", "wind")}}
    return {OVERVIEW: overview, DETAIL: {**terrain, **{name: values.reshape(YEARS * WEEKS, *shape) for name, values in detail.items()}}}


def tiles(region, values):
    """{tile ID: gzip body} of every tile that has a cell with data."""
    from pmtiles.tile import zxy_to_tileid

    rows = region.rows[region.pad:len(region.rows) - region.pad]
    cols = region.cols[region.pad:len(region.cols) - region.pad]
    result = {}
    for zoom, (block_cols, block_rows, layout) in LEVELS.items():
        for y in range(rows[0] // block_rows, rows[-1] // block_rows + 1):
            for x in range(cols[0] // block_cols, cols[-1] // block_cols + 1):
                r = np.flatnonzero(rows // block_rows == y)
                c = np.flatnonzero(cols // block_cols == x)
                body, data = [], False
                for name, count in layout:
                    kind, missing = CODES[name][:2]
                    plane = np.full((count, block_rows, block_cols), missing, kind)
                    plane[:, (rows[r] % block_rows)[:, None], cols[c] % block_cols] = encode(name, values[zoom][name][:, r][:, :, c])
                    data |= bool((plane != missing).any())
                    body.append(plane.tobytes())
                if data:
                    result[zxy_to_tileid(zoom, x, y)] = gzip.compress(b"".join(body), 9, mtime=0)
    return result


def bake(bounds, first_year, source, output, key=None):
    """Write the archive; return the tile count of each zoom."""
    from pmtiles.tile import Compression, TileType, tileid_to_zxy
    from pmtiles.writer import Writer

    region = Region(bounds)
    weekly, monthly, rose = climate(region, first_year, source)
    archive = tiles(region, planes(region, first_year, weekly, monthly, rose, orography(region, key)))
    west, south, east, north = bounds
    e7 = lambda value: round(value * 1e7)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=output.parent, prefix=".climate-", delete=False) as stream:
        try:
            writer = Writer(stream)
            for tile_id in sorted(archive):
                writer.write_tile(tile_id, archive[tile_id])
            writer.finalize({
                "tile_type": TileType.UNKNOWN, "tile_compression": Compression.GZIP,
                "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                "center_zoom": OVERVIEW, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
            }, {
                "first_year": first_year, "years": YEARS, "source": "era5-land",
                "attribution": f"Contains modified Copernicus Climate Change Service information {first_year + YEARS}: "
                               f"ERA5-Land (doi:{DOI})",
                "wet_day_mm": WET_MM, "rain_factors": list(RAIN_FACTORS), "wind_factor": WIND_FACTOR,
                "inputs": {"doi": DOI, "orography_sha256": OROGRAPHY[1], "chunks": source.fingerprint()},
            })
            stream.flush()
            os.replace(stream.name, output)
        except BaseException:
            os.unlink(stream.name)
            raise
    zooms = [tileid_to_zxy(tile_id)[0] for tile_id in archive]
    return {zoom: zooms.count(zoom) for zoom in LEVELS}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("region", help="region name: the recipe in tools/planner-regions and the default output folder")
    parser.add_argument("--bounds", type=maps.bounds, help="west,south,east,north instead of the recipe bounds")
    parser.add_argument("--first-year", type=int, help="the first of the ten years; default: the recipe's `climate.first_year`")
    parser.add_argument("--output", type=Path, help="default: ~/.cache/obc/planner/REGION/maps/climate.pmtiles")
    parser.add_argument("--check", action="store_true", help="bake from the cache only and compare with the output")
    args = parser.parse_args()
    recipe = json.loads((RECIPES / f"{args.region}.json").read_text()) if (RECIPES / f"{args.region}.json").exists() else {}
    bounds = args.bounds or recipe["bounds"]
    first_year = args.first_year or recipe.get("climate", {}).get("first_year")
    if first_year is None:
        parser.error("Pin the first year with --first-year or the recipe field `climate.first_year`")
    output = args.output or Path.home() / ".cache/obc/planner" / args.region / "maps/climate.pmtiles"
    final_hour = hour(dt.date(first_year + YEARS, 1, 1)) + FINAL_AFTER_DAYS * 24
    start = time.monotonic()
    if args.check:
        with tempfile.TemporaryDirectory() as directory:
            bake(bounds, first_year, Source(final_hour), Path(directory) / output.name)
            if (Path(directory) / output.name).read_bytes() != output.read_bytes():
                sys.exit(f"{output} differs from a bake of the cached sources")
        print(f"{output} equals a bake of the cached sources")
        return
    key = token()
    source = Source(final_hour, key)
    counts = bake(bounds, first_year, source, output, key)
    print(json.dumps({"output": str(output), "bytes": output.stat().st_size, "tiles": counts,
                      "downloaded_bytes": source.downloaded, "source_chunks": sum(map(len, source.digests.values())),
                      "seconds": round(time.monotonic() - start)}))


if __name__ == "__main__":
    main()
