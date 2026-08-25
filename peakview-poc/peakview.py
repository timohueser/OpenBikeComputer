#!/usr/bin/env python3
"""Render a tiny PeakFinder-like view from public DEM and OpenStreetMap data."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import time
from datetime import date, datetime, time as dtime, timedelta
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path

import numpy as np
import requests
from PIL import Image, ImageDraw


WIDTH, HEIGHT = 240, 268
STRIP_WIDTH = 720
FONT_PATH = Path(__file__).resolve().parent.parent / "firmware" / "obc-render" / "fonts" / "terminus" / "ter_u24b.raw"
CELL_W, CELL_H = 12, 24
COMPASS_BOTTOM = 34
HUD_HEIGHT = 54
WINDS = ("N", "NE", "E", "SE", "S", "SW", "W", "NW")
DEM_ZOOM = 11
EARTH_RADIUS_M = 6_371_000.0
REFRACTION_K = 0.13
USER_AGENT = "OpenBikeComputer-PeakView-PoC/0.1 (+https://github.com/OpenBikeComputer)"
TERRARIUM_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
OVERPASS_URLS = (
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
)

SKY = (255, 255, 255)
TERRAIN = (85, 170, 85)
INK = (85, 85, 85)
BLACK = (0, 0, 0)
AMBER = (255, 170, 0)
PAPER = (255, 255, 255)
ROUTE = (255, 0, 255)  # the device's route overlay magenta (obc-app palette)

# Ordered nearest to farthest. RGB222 limits a same-hue ramp to four coarse
# lightness steps, but the white background keeps all four readable.
LAYER_PALETTES = {
    1: (TERRAIN,),
    3: ((0, 85, 0), (85, 170, 85), (170, 255, 170)),
    4: ((0, 85, 0), (0, 170, 0), (85, 170, 85), (170, 255, 170)),
}

PRESETS = {
    "gornergrat": (45.9834, 7.7854, 220.0, "Gornergrat"),
    "kleine-scheidegg": (46.5850, 7.9610, 150.0, "Kleine Scheidegg"),
    "grossglockner": (47.0745, 12.7530, 250.0, "Kaiser-Franz-Josefs-Hoehe"),
    "feldberg": (47.8740, 8.0040, 180.0, "Feldberg"),
}


@dataclass
class Peak:
    name: str
    lat: float
    lon: float
    elevation: float
    distance: float = 0.0
    azimuth: float = 0.0
    angle: float = 0.0
    offset: float = 0.0
    score: float = 0.0
    wikidata: str | None = None
    prominence: float | None = None


def cache_root() -> Path:
    path = Path(__file__).resolve().parent / "cache"
    path.mkdir(parents=True, exist_ok=True)
    return path


def angular_delta(angle: np.ndarray | float, centre: float) -> np.ndarray | float:
    return (angle - centre + 180.0) % 360.0 - 180.0


def distances(max_range_m: float) -> np.ndarray:
    result = []
    distance = 30.0
    while distance <= max_range_m:
        result.append(distance)
        distance += min(200.0, max(30.0, distance * 0.004))
    return np.asarray(result, dtype=np.float64)


def destination_grid(lat: float, lon: float, bearings: np.ndarray, ranges: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    radians = np.deg2rad(bearings)[None, :]
    distance = ranges[:, None]
    north = distance * np.cos(radians)
    east = distance * np.sin(radians)
    lats = lat + np.rad2deg(north / EARTH_RADIUS_M)
    lons = lon + np.rad2deg(east / (EARTH_RADIUS_M * math.cos(math.radians(lat))))
    return lats, lons


class TerrariumDEM:
    def __init__(self, zoom: int, offline: bool):
        self.zoom = zoom
        self.offline = offline
        self.root = cache_root() / "dem" / str(zoom)
        self.tiles: dict[tuple[int, int], np.ndarray] = {}

    def _path(self, x: int, y: int) -> Path:
        return self.root / str(x) / f"{y}.png"

    def _download(self, tile: tuple[int, int]) -> None:
        x, y = tile
        path = self._path(x, y)
        if path.exists():
            return
        if self.offline:
            raise RuntimeError(f"DEM tile is not cached: z{self.zoom}/{x}/{y}")
        url = TERRARIUM_URL.format(z=self.zoom, x=x, y=y)
        last_error: Exception | None = None
        for attempt in range(3):
            try:
                response = requests.get(url, headers={"User-Agent": USER_AGENT}, timeout=30)
                response.raise_for_status()
                Image.open(BytesIO(response.content)).verify()
                path.parent.mkdir(parents=True, exist_ok=True)
                temporary = path.with_suffix(".tmp")
                temporary.write_bytes(response.content)
                temporary.replace(path)
                return
            except Exception as error:  # network failures are retried and then reported together
                last_error = error
                time.sleep(0.5 * (attempt + 1))
        raise RuntimeError(f"Could not fetch DEM tile {url}: {last_error}")

    def _global_pixels(self, lats: np.ndarray, lons: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        scale = (1 << self.zoom) * 256.0
        clipped = np.clip(lats, -85.05112878, 85.05112878)
        x = (lons + 180.0) / 360.0 * scale
        latitude = np.deg2rad(clipped)
        y = (1.0 - np.arcsinh(np.tan(latitude)) / math.pi) / 2.0 * scale
        return x, y

    def preload(self, lats: np.ndarray, lons: np.ndarray) -> None:
        px, py = self._global_pixels(lats, lons)
        max_tile = (1 << self.zoom) - 1
        required: set[tuple[int, int]] = set()
        for dx, dy in ((0, 0), (1, 0), (0, 1), (1, 1)):
            ix = np.floor(px).astype(np.int64) + dx
            iy = np.floor(py).astype(np.int64) + dy
            tx = (ix // 256) % (1 << self.zoom)
            ty = np.clip(iy // 256, 0, max_tile)
            required.update(zip(tx.ravel().tolist(), ty.ravel().tolist()))

        missing = [tile for tile in sorted(required) if not self._path(*tile).exists()]
        if missing:
            print(f"Fetching {len(missing)} DEM tiles (cached after this run) ...")
            with ThreadPoolExecutor(max_workers=12) as executor:
                futures = {executor.submit(self._download, tile): tile for tile in missing}
                for future in as_completed(futures):
                    future.result()

        for tile in required:
            path = self._path(*tile)
            rgb = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32)
            self.tiles[tile] = rgb[..., 0] * 256.0 + rgb[..., 1] + rgb[..., 2] / 256.0 - 32768.0

    def sample(self, lats: np.ndarray, lons: np.ndarray) -> np.ndarray:
        px, py = self._global_pixels(lats, lons)
        x0 = np.floor(px).astype(np.int64)
        y0 = np.floor(py).astype(np.int64)
        fx = px - x0
        fy = py - y0

        def pixels(ix: np.ndarray, iy: np.ndarray) -> np.ndarray:
            result = np.empty(ix.shape, dtype=np.float32)
            tile_x = (ix // 256) % (1 << self.zoom)
            tile_y = iy // 256
            keys = np.stack((tile_x.ravel(), tile_y.ravel()), axis=1)
            for tx, ty in np.unique(keys, axis=0):
                mask = (tile_x == tx) & (tile_y == ty)
                result[mask] = self.tiles[(int(tx), int(ty))][iy[mask] % 256, ix[mask] % 256]
            return result

        a = pixels(x0, y0)
        b = pixels(x0 + 1, y0)
        c = pixels(x0, y0 + 1)
        d = pixels(x0 + 1, y0 + 1)
        return a * (1 - fx) * (1 - fy) + b * fx * (1 - fy) + c * (1 - fx) * fy + d * fx * fy


def horizon(dem: TerrariumDEM, lat: float, lon: float, bearings: np.ndarray, max_range_m: float,
            eye_height: float, layer_count: int) -> tuple[np.ndarray, np.ndarray, np.ndarray, float]:
    ranges = distances(max_range_m)
    lats, lons = destination_grid(lat, lon, bearings, ranges)
    dem.preload(np.concatenate((lats.ravel(), [lat])), np.concatenate((lons.ravel(), [lon])))
    observer_elevation = float(dem.sample(np.asarray([lat]), np.asarray([lon]))[0])
    terrain = dem.sample(lats, lons)
    drop = ranges[:, None] ** 2 / (2.0 * EARTH_RADIUS_M) * (1.0 - REFRACTION_K)
    angles = np.rad2deg(np.arctan2(terrain - (observer_elevation + eye_height) - drop, ranges[:, None]))

    # Cluster visible intermediate ridge crests by distance. This responds to a
    # view's actual foreground/background structure instead of its maximum range.
    local_maxima = (angles[1:-1] >= angles[:-2]) & (angles[1:-1] > angles[2:])
    nearer_horizon = np.maximum.accumulate(angles, axis=0)[:-2]
    visible_crests = local_maxima & (angles[1:-1] >= nearer_horizon - 0.1)
    crest_samples = np.nonzero(visible_crests)[0] + 1
    crest_distances = ranges[crest_samples]
    if len(crest_distances) < layer_count:
        crest_distances = ranges[np.argmax(angles, axis=0)]

    quantiles = (np.arange(layer_count) + 0.5) / layer_count
    centres = np.quantile(crest_distances, quantiles)
    for _ in range(20):
        assignments = np.argmin(abs(crest_distances[:, None] - centres[None, :]), axis=1)
        updated = np.asarray([
            np.mean(crest_distances[assignments == index])
            if np.any(assignments == index) else centre
            for index, centre in enumerate(centres)
        ])
        if np.allclose(updated, centres):
            break
        centres = updated
    raw_edges = np.searchsorted(ranges, (centres[:-1] + centres[1:]) / 2.0, side="right")

    # Integer slice edges keep every ray-march sample in exactly one band.
    sample_edges = [0]
    for position, raw_edge in enumerate(np.atleast_1d(raw_edges), start=1):
        remaining_bands = layer_count - position
        edge = max(sample_edges[-1] + 1, int(raw_edge))
        sample_edges.append(min(edge, len(ranges) - remaining_bands))
    sample_edges.append(len(ranges))

    layers = []
    for start, end in zip(sample_edges[:-1], sample_edges[1:]):
        layers.append(np.max(angles[start:end], axis=0))
    distance_edges = np.asarray(
        [0.0] + [(ranges[index - 1] + ranges[index]) / 2.0 for index in sample_edges[1:-1]] + [max_range_m]
    )
    return np.max(angles, axis=0), np.stack(layers), distance_edges, observer_elevation


def cached_json(path: Path, loader, offline: bool):
    if path.exists():
        return json.loads(path.read_text(encoding="utf-8"))
    if offline:
        raise RuntimeError(f"Downloaded data is not cached: {path}")
    value = loader()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")
    return value


def fetch_peak_data(lat: float, lon: float, max_range_m: float, offline: bool) -> dict:
    key = hashlib.sha1(f"{lat:.5f},{lon:.5f},{max_range_m:.0f}".encode()).hexdigest()[:16]
    path = cache_root() / "overpass" / f"peaks-{key}.json"

    def load():
        query = f'[out:json][timeout:90];node["natural"="peak"]["name"](around:{max_range_m:.0f},{lat},{lon});out body;'
        errors = []
        for endpoint in OVERPASS_URLS:
            try:
                response = requests.post(endpoint, data={"data": query}, headers={"User-Agent": USER_AGENT}, timeout=120)
                response.raise_for_status()
                return response.json()
            except Exception as error:
                errors.append(f"{endpoint}: {error}")
        raise RuntimeError("Could not fetch peaks from Overpass: " + "; ".join(errors))

    return cached_json(path, load, offline)


def parse_elevation(value: object) -> float | None:
    if value is None:
        return None
    text = str(value).strip().replace("\u202f", "").replace(" ", "")
    match = re.search(r"[-+]?\d[\d,.]*", text)
    if not match:
        return None
    number = match.group(0)
    if "," in number and "." not in number:
        tail = number.rsplit(",", 1)[1]
        number = number.replace(",", "") if len(tail) == 3 else number.replace(",", ".")
    elif "," in number:
        number = number.replace(",", "")
    try:
        elevation = float(number)
        return elevation if -500.0 < elevation < 10_000.0 else None
    except ValueError:
        return None


def peak_geometry(peaks: list[Peak], lat: float, lon: float) -> None:
    lat_scale = EARTH_RADIUS_M * math.pi / 180.0
    lon_scale = lat_scale * math.cos(math.radians(lat))
    for peak in peaks:
        north = (peak.lat - lat) * lat_scale
        east = (peak.lon - lon) * lon_scale
        peak.distance = math.hypot(north, east)
        peak.azimuth = math.degrees(math.atan2(east, north)) % 360.0


def wikidata_prominence(qid: str, offline: bool) -> float | None:
    path = cache_root() / "wikidata" / f"{qid}.json"

    def load():
        url = f"https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"
        response = requests.get(url, headers={"User-Agent": USER_AGENT}, timeout=30)
        response.raise_for_status()
        return response.json()

    try:
        data = cached_json(path, load, offline)
        claims = data["entities"][qid]["claims"].get("P2660", [])
        amounts = [abs(float(item["mainsnak"]["datavalue"]["value"]["amount"])) for item in claims]
        return max(amounts, default=None)
    except (KeyError, TypeError, ValueError, RuntimeError, requests.RequestException):
        return None


def prepare_peaks(data: dict, lat: float, lon: float, observer_elevation: float, max_range_m: float,
                  heading: float, half_fov: float, horizon_angles: np.ndarray, bearings: np.ndarray,
                  tolerance: float, use_wikidata: bool, offline: bool) -> list[Peak]:
    peaks = []
    for element in data.get("elements", []):
        tags = element.get("tags", {})
        elevation = parse_elevation(tags.get("ele"))
        if elevation is None:
            continue
        peaks.append(Peak(tags["name"], float(element["lat"]), float(element["lon"]), elevation,
                          wikidata=tags.get("wikidata"), prominence=parse_elevation(tags.get("prominence"))))
    peak_geometry(peaks, lat, lon)
    peaks = [peak for peak in peaks if 100.0 < peak.distance <= max_range_m]
    if not peaks:
        return []

    elevations = np.asarray([peak.elevation for peak in peaks])
    peak_lats = np.asarray([peak.lat for peak in peaks])
    peak_lons = np.asarray([peak.lon for peak in peaks])
    for peak in peaks:
        higher = elevations > peak.elevation
        if np.any(higher):
            north = (peak_lats[higher] - peak.lat) * EARTH_RADIUS_M * math.pi / 180.0
            east = (peak_lons[higher] - peak.lon) * EARTH_RADIUS_M * math.pi / 180.0 * math.cos(math.radians(peak.lat))
            isolation_km = min(20.0, float(np.min(np.hypot(north, east))) / 1000.0)
        else:
            isolation_km = 20.0
        peak.score = peak.elevation * max(0.1, isolation_km)
        if peak.prominence:
            peak.score = peak.prominence * 100.0
        if use_wikidata and peak.wikidata:
            prominence = wikidata_prominence(peak.wikidata, offline)
            if prominence:
                peak.score = prominence * 100.0

        peak.offset = float(angular_delta(peak.azimuth, heading))
        drop = peak.distance ** 2 / (2.0 * EARTH_RADIUS_M) * (1.0 - REFRACTION_K)
        peak.angle = math.degrees(math.atan2(peak.elevation - observer_elevation - drop, peak.distance))

    visible = []
    bearing_unwrapped = heading + np.asarray([angular_delta(value, heading) for value in bearings])
    for peak in peaks:
        if not (-half_fov <= peak.offset < half_fov):
            continue
        skyline = float(np.interp(heading + peak.offset, bearing_unwrapped, horizon_angles))
        if peak.angle >= skyline - tolerance:
            visible.append(peak)
    return visible


def quantize_rgb222(image: Image.Image) -> Image.Image:
    pixels = np.asarray(image.convert("RGB"), dtype=np.uint8)
    return Image.fromarray(((pixels >> 6) * 85).astype(np.uint8), "RGB")


class DeviceFont:
    """The device's Label text tier: Terminus 12x24 bold, read from the firmware's glyph strip.

    The strip is 1bpp MSB-first, 16 glyphs per row, in the firmware's `latin` charset order
    (ASCII, Latin-1 Supplement, Latin Extended-A), so this renders the exact pixels the
    device would.
    """

    def __init__(self, path: Path):
        bits = np.unpackbits(np.frombuffer(path.read_bytes(), dtype=np.uint8))
        strip = bits.reshape(-1, 16 * CELL_W).astype(bool)
        self.glyphs = [
            strip[row * CELL_H:(row + 1) * CELL_H, col * CELL_W:(col + 1) * CELL_W]
            for row in range(strip.shape[0] // CELL_H)
            for col in range(16)
        ]

    @staticmethod
    def _index(char: str) -> int:
        point = ord(char)
        if 0x20 <= point <= 0x7F:
            return point - 0x20
        if 0xA0 <= point <= 0xFF:
            return point - 0xA0 + 96
        if 0x100 <= point <= 0x17F:
            return point - 0x100 + 192
        return ord("?") - 0x20

    def mask(self, text: str) -> np.ndarray:
        if not text:
            return np.zeros((CELL_H, 0), dtype=bool)
        return np.concatenate([self.glyphs[self._index(char)] for char in text], axis=1)

    def trimmed(self, text: str) -> np.ndarray:
        """Glyph mask with empty leading/trailing rows removed, for tight boxes and rotation."""
        mask = self.mask(text)
        rows = np.nonzero(mask.any(axis=1))[0]
        return mask[rows[0]:rows[-1] + 1] if rows.size else mask[:0]


def paint(image: Image.Image, mask: np.ndarray, xy: tuple[int, int], colour: tuple[int, int, int],
          halo: tuple[int, int, int] | None = None) -> None:
    """Blit a glyph mask; an optional 1 px 8-neighbour halo keeps text legible over terrain."""
    if mask.size == 0:
        return
    if halo is not None:
        padded = np.pad(mask, 1)
        dilated = np.zeros_like(padded)
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                dilated |= np.roll(padded, (dy, dx), (0, 1))
        image.paste(halo, (xy[0] - 1, xy[1] - 1), Image.fromarray(dilated.astype(np.uint8) * 255, "L"))
    image.paste(colour, xy, Image.fromarray(mask.astype(np.uint8) * 255, "L"))


def short_name(name: str, limit: int) -> str:
    """Device-width peak name: first alternative, no parenthetical, whole words up to `limit` glyphs."""
    name = name.split("/")[0].split("(")[0].strip()
    if len(name) <= limit:
        return name
    cut = name[:limit]
    word_cut = cut.rsplit(" ", 1)[0] if " " in cut else cut
    return word_cut if len(word_cut) >= 8 else cut.rstrip()


def wind_name(azimuth: float) -> str:
    return WINDS[int(((azimuth + 22.5) % 360.0) // 45.0)]


def solar_position(lat: float, lon: float, when_utc: datetime) -> tuple[float, float]:
    """Sun azimuth (degrees clockwise from north) and elevation, NOAA's approximation."""
    day = when_utc.timetuple().tm_yday
    hours = when_utc.hour + when_utc.minute / 60.0
    gamma = 2.0 * math.pi / 365.0 * (day - 1 + (hours - 12.0) / 24.0)
    eqtime = 229.18 * (0.000075 + 0.001868 * math.cos(gamma) - 0.032077 * math.sin(gamma)
                       - 0.014615 * math.cos(2 * gamma) - 0.040849 * math.sin(2 * gamma))
    decl = (0.006918 - 0.399912 * math.cos(gamma) + 0.070257 * math.sin(gamma)
            - 0.006758 * math.cos(2 * gamma) + 0.000907 * math.sin(2 * gamma)
            - 0.002697 * math.cos(3 * gamma) + 0.00148 * math.sin(3 * gamma))
    hour_angle = math.radians((hours * 60.0 + eqtime + 4.0 * lon) / 4.0 - 180.0)
    lat_r = math.radians(lat)
    cos_zenith = math.sin(lat_r) * math.sin(decl) + math.cos(lat_r) * math.cos(decl) * math.cos(hour_angle)
    zenith = math.acos(max(-1.0, min(1.0, cos_zenith)))
    azimuth = (math.degrees(math.atan2(math.sin(hour_angle),
               math.cos(hour_angle) * math.sin(lat_r) - math.tan(decl) * math.cos(lat_r))) + 180.0) % 360.0
    return azimuth, 90.0 - math.degrees(zenith)


def sun_track(lat: float, lon: float, local: datetime, tz_hours: float, heading: float, fov: float,
              horizon_angles: np.ndarray, width: int) -> tuple[tuple[float, float], tuple[str, float, str] | None]:
    """Current sun position, plus the next minute the sun crosses the visible skyline.

    Scans forward a day in one-minute steps; a crossing only counts while the sun's azimuth is
    inside the current field of view, because that is the only horizon this render computed.
    On device the cached 360-degree horizon profile makes this a whole-day, whole-horizon scan.
    """
    utc = local - timedelta(hours=tz_hours)

    def skyline_at(azimuth: float) -> float | None:
        x = (angular_delta(azimuth, heading) + fov / 2.0) * width / fov
        return float(horizon_angles[int(x)]) if 0 <= x < width else None

    event = None
    previous = None
    for minute in range(24 * 60):
        azimuth, elevation = solar_position(lat, lon, utc + timedelta(minutes=minute))
        skyline = skyline_at(azimuth) if elevation > -8.0 else None
        visible = None if skyline is None else elevation > skyline
        if previous is not None and visible is not None and visible != previous:
            when = (local + timedelta(minutes=minute)).strftime("%H:%M")
            event = (when, azimuth, "sets behind the ridge" if previous else "clears the ridge")
            break
        previous = visible
    return solar_position(lat, lon, utc), event


def route_crossing(dem: TerrariumDEM, lat: float, lon: float, observer_elevation: float, eye_height: float,
                   bearing: float, max_range_m: float) -> tuple[float, float] | None:
    """Where a mock straight route along `bearing` disappears over the skyline: (bearing, metres).

    On device this walks the active route's real polyline instead; the crest search is the same.
    The mock ride stops at 25 km — a plausible route horizon, not the DEM's.
    """
    ranges = np.arange(200.0, min(max_range_m, 25_000.0), 60.0)
    lats, lons = destination_grid(lat, lon, np.asarray([bearing]), ranges)
    try:
        dem.preload(lats.ravel(), lons.ravel())
        elevation = dem.sample(lats, lons)[:, 0]
    except (RuntimeError, KeyError):
        return None  # tiles beyond the rendered fan are not cached in --offline runs
    drop = ranges ** 2 / (2.0 * EARTH_RADIUS_M) * (1.0 - REFRACTION_K)
    angles = np.degrees(np.arctan2(elevation - (observer_elevation + eye_height) - drop, ranges))
    crest = int(np.argmax(angles))
    return bearing, float(ranges[crest])


def render(horizon_angles: np.ndarray, layer_horizons: np.ndarray, peaks: list[Peak],
           heading: float, fov: float, width: int, max_labels: int, min_score: float, peak_step: int,
           sun_now: tuple[float, float] | None, sun_event: tuple[str, float, str] | None,
           route_cross: tuple[float, float] | None) -> tuple[Image.Image, list[Peak], Peak | None]:
    font = DeviceFont(FONT_PATH)
    image = Image.new("RGB", (width, HEIGHT), SKY)
    draw = ImageDraw.Draw(image)
    compass_bottom, hud_top = COMPASS_BOTTOM, HEIGHT - HUD_HEIGHT

    low = float(np.min(horizon_angles))
    high = float(np.max(horizon_angles))
    span = high - low
    if span < 0.5:
        angle_bottom, angle_top = -20.0, 20.0
    else:
        angle_bottom = low - max(1.0, span * 0.12)
        angle_top = high + max(3.0, span * 0.42)

    def y_for(angle: float | np.ndarray):
        return hud_top - (np.asarray(angle) - angle_bottom) / (angle_top - angle_bottom) * (hud_top - compass_bottom)

    skyline_y = np.clip(np.rint(y_for(horizon_angles)), compass_bottom, hud_top).astype(int)
    palette = LAYER_PALETTES[len(layer_horizons)]
    outline = INK if len(layer_horizons) == 1 else BLACK
    for layer, colour in zip(reversed(layer_horizons), reversed(palette)):
        layer_y = np.clip(np.rint(y_for(layer)), compass_bottom, hud_top).astype(int)
        polygon = [(0, hud_top)] + list(zip(range(width), layer_y.tolist())) + [(width - 1, hud_top)]
        draw.polygon(polygon, fill=colour)
        draw.line(list(zip(range(width), layer_y.tolist())), fill=outline, width=1)
    draw.line(list(zip(range(width), skyline_y.tolist())), fill=outline, width=1)

    degrees_per_pixel = fov / width
    left_bearing = heading - fov / 2.0
    centre = width // 2
    heading_mask = font.trimmed(f"{heading % 360:03.0f}\N{DEGREE SIGN}")
    heading_box = (centre - heading_mask.shape[1] // 2 - 2, 0,
                   centre + (heading_mask.shape[1] + 1) // 2 + 2, heading_mask.shape[0] + 4)

    tick = math.ceil(left_bearing / 10.0) * 10
    while tick < left_bearing + fov:
        x = int(round((tick - left_bearing) / degrees_per_pixel))
        bearing = int(round(tick)) % 360
        major = bearing % 45 == 0
        draw.line((x, 0, x, 6 if major else 3), fill=INK)
        if major:
            mask = font.trimmed(WINDS[bearing // 45])
            left = x - mask.shape[1] // 2
            clear_of_heading = left + mask.shape[1] < heading_box[0] - 2 or left > heading_box[2] + 2
            if clear_of_heading and 0 <= left and left + mask.shape[1] <= width:
                paint(image, mask, (left, 8), INK, halo=PAPER)
        tick += 10

    # The heading line goes down first so the readout box and label halos mask it.
    draw.line((centre, 0, centre, hud_top - 1), fill=AMBER)
    draw.rectangle(heading_box, fill=PAPER)
    paint(image, heading_mask, (heading_box[0] + 2, 2), INK)

    candidates = sorted((peak for peak in peaks if peak.score >= min_score), key=lambda peak: peak.score, reverse=True)
    separated: list[Peak] = []
    min_separation = max(0.35, degrees_per_pixel * 4)
    for peak in candidates:
        if all(abs(peak.offset - other.offset) >= min_separation for other in separated):
            separated.append(peak)

    selected_peak = None
    if separated:
        focus_pool = separated[:20]
        nearest = min(separated, key=lambda peak: abs(peak.offset))
        if nearest not in focus_pool:
            focus_pool.append(nearest)
        focus_pool.sort(key=lambda peak: peak.offset)
        centre_index = min(range(len(focus_pool)), key=lambda index: abs(focus_pool[index].offset))
        selected_peak = focus_pool[(centre_index + peak_step) % len(focus_pool)]

    # Overlays claim space in glanceability order — selected peak, sun, route — and the generic
    # name labels fill whatever room is left.
    rects: list[tuple[int, int, int, int]] = [(heading_box[0] - 2, heading_box[1], heading_box[2] + 2, heading_box[3] + 2)]

    def collides(box: tuple[int, int, int, int]) -> bool:
        return any(box[0] <= r[2] + 2 and box[2] >= r[0] - 2 and box[1] <= r[3] + 1 and box[3] >= r[1] - 1
                   for r in rects)

    def place_text(mask: np.ndarray, spots: list[tuple[int, int]]) -> None:
        boxes = [(x, y, x + mask.shape[1], y + mask.shape[0]) for x, y in spots]
        boxes = [box for box in boxes
                 if 1 <= box[0] and box[2] < width - 1 and compass_bottom + 2 <= box[1] and box[3] < hud_top - 1]
        if not boxes:
            return
        box = next((candidate for candidate in boxes if not collides(candidate)), boxes[0])
        rects.append(box)
        paint(image, mask, (box[0], box[1]), INK, halo=PAPER)

    def x_for(azimuth: float) -> int | None:
        x = int(round((angular_delta(azimuth, heading) + fov / 2.0) / degrees_per_pixel))
        return x if 6 <= x < width - 6 else None

    if selected_peak:
        selected_x = int(np.clip(round((selected_peak.offset + fov / 2.0) / degrees_per_pixel), 0, width - 1))
        selected_y = int(skyline_y[selected_x])
        draw.polygon(((selected_x, selected_y - 1), (selected_x - 4, selected_y - 7),
                      (selected_x + 4, selected_y - 7)), fill=AMBER)
        rects.append((selected_x - 5, selected_y - 8, selected_x + 5, selected_y))

    if sun_now:
        sun_x = x_for(sun_now[0])
        sun_y = int(round(float(y_for(sun_now[1]))))
        if sun_x is not None and compass_bottom + 8 <= sun_y < hud_top - 8 \
                and sun_now[1] > horizon_angles[sun_x]:
            draw.ellipse((sun_x - 3, sun_y - 3, sun_x + 3, sun_y + 3), fill=AMBER)
            for step in range(8):
                dx, dy = math.cos(step * math.pi / 4.0), math.sin(step * math.pi / 4.0)
                draw.line((sun_x + round(5 * dx), sun_y + round(5 * dy),
                           sun_x + round(7 * dx), sun_y + round(7 * dy)), fill=AMBER)
            rects.append((sun_x - 8, sun_y - 8, sun_x + 8, sun_y + 8))

    if sun_event and abs(angular_delta(sun_event[1], heading)) <= fov / 2.0:
        # Clamp an in-view event to the frame edge rather than dropping a sunset pixels away.
        event_x = int(np.clip(round((angular_delta(sun_event[1], heading) + fov / 2.0) / degrees_per_pixel),
                              6, width - 7))
        event_y = int(skyline_y[event_x])
        # A half-sunk sun disc on the ridge line, with the crossing time beside it.
        draw.pieslice((event_x - 5, event_y - 5, event_x + 5, event_y + 5), 180, 360, fill=AMBER)
        rects.append((event_x - 6, event_y - 6, event_x + 6, event_y + 1))
        mask = font.trimmed(sun_event[0])
        half = mask.shape[0] // 2
        place_text(mask, [(event_x + 9, event_y - half), (event_x - 9 - mask.shape[1], event_y - half),
                          (min(max(1, event_x - mask.shape[1] // 2), width - mask.shape[1] - 1),
                           event_y - 12 - mask.shape[0])])

    if route_cross:
        route_x = x_for(route_cross[0])
        if route_x is not None:
            route_y = int(skyline_y[route_x])
            # The map overlay's travel chevrons, climbing over the pass in the route magenta.
            for lift in (0, 7):
                draw.polygon(((route_x - 5, route_y + 3 + lift), (route_x, route_y - 3 + lift),
                              (route_x + 5, route_y + 3 + lift), (route_x + 5, route_y + 6 + lift),
                              (route_x, route_y + lift), (route_x - 5, route_y + 6 + lift)), fill=ROUTE)
            rects.append((route_x - 6, route_y - 4, route_x + 6, route_y + 14))
            kilometres = route_cross[1] / 1000.0
            mask = font.trimmed(f"{kilometres:.1f} km" if kilometres < 9.95 else f"{kilometres:.0f} km")
            half = mask.shape[0] // 2
            place_text(mask, [(min(max(1, route_x - mask.shape[1] // 2), width - mask.shape[1] - 1),
                               route_y - 12 - mask.shape[0]),
                              (route_x + 9, route_y - half), (route_x - 9 - mask.shape[1], route_y - half)])

    # Vertical labels rise from each summit; a name is cut to the sky room above its own peak.
    labelled: list[Peak] = []
    columns: list[tuple[int, int]] = []
    used_names: set[str] = set()
    for peak in separated:
        if peak is selected_peak:
            continue
        if len(labelled) >= max_labels:
            break
        x = int(round((peak.offset + fov / 2.0) / degrees_per_pixel))
        if not 2 <= x < width - 2:
            continue
        marker_y = int(skyline_y[x])
        label = short_name(peak.name, 14)
        if len(label) < 3 or label in used_names:
            continue
        mask = np.rot90(font.trimmed(label))
        height, glyph_width = mask.shape
        left = min(max(1, x - glyph_width // 2), width - glyph_width - 1)
        if any(left <= stop + 2 and left + glyph_width >= start - 2 for start, stop in columns):
            continue
        # Rise from the summit; when the sky is too short, slide down over the terrain — the
        # halo keeps the name readable there.
        top = max(compass_bottom + 2, marker_y - 8 - height)
        if collides((left, top, left + glyph_width, top + height)):
            continue
        paint(image, mask, (left, top), INK, halo=PAPER)
        if top + height < marker_y - 2:
            draw.line((x, marker_y - 2, x, top + height + 1), fill=INK)
        columns.append((left, left + glyph_width))
        rects.append((left, top, left + glyph_width, top + height))
        used_names.add(label)
        labelled.append(peak)

    draw.rectangle((0, hud_top, width - 1, HEIGHT - 1), fill=PAPER)
    draw.line((0, hud_top, width - 1, hud_top), fill=INK)
    if selected_peak:
        # The same amber triangle as the skyline marker links the panel to the summit it describes.
        draw.polygon(((3, hud_top + 11), (13, hud_top + 11), (8, hud_top + 17)), fill=AMBER)
        name = short_name(selected_peak.name, (width - 20) // CELL_W)
        paint(image, font.mask(name), (18, hud_top + 3), BLACK)
        kilometres = selected_peak.distance / 1000.0
        distance_text = f"{kilometres:.1f} km" if kilometres < 9.95 else f"{kilometres:.0f} km"
        metrics = f"{selected_peak.elevation:.0f} m  {distance_text}  {wind_name(selected_peak.azimuth)}"
        paint(image, font.mask(metrics), (18, hud_top + 27), INK)
    else:
        paint(image, font.mask("No named peak"), (18, hud_top + 3), INK)
        paint(image, font.mask("in view"), (18, hud_top + 27), INK)
    return quantize_rgb222(image), labelled, selected_peak


def output_paths(path: Path, scale: int) -> tuple[Path, Path]:
    native = path.with_suffix(".png")
    enlarged = native.with_name(f"{native.stem}_{scale}x.png")
    return native, enlarged


def save_pair(image: Image.Image, path: Path, scale: int) -> None:
    native, enlarged = output_paths(path, scale)
    native.parent.mkdir(parents=True, exist_ok=True)
    image.save(native, optimize=True)
    image.resize((image.width * scale, image.height * scale), Image.Resampling.NEAREST).save(enlarged, optimize=True)
    print(f"Saved {native} and {enlarged}")


def render_view(args, lat: float, lon: float, heading: float, width: int, fov: float, path: Path) -> None:
    degrees_per_pixel = fov / width
    offsets = (np.arange(width) - width / 2.0) * degrees_per_pixel
    bearings = (heading + offsets) % 360.0
    dem = TerrariumDEM(DEM_ZOOM, args.offline)
    horizon_angles, layer_horizons, layer_edges, observer_elevation = horizon(
        dem, lat, lon, bearings, args.max_range * 1000.0, args.eye_height, args.layers
    )
    data = fetch_peak_data(lat, lon, args.max_range * 1000.0, args.offline)
    visible = prepare_peaks(data, lat, lon, observer_elevation, args.max_range * 1000.0, heading, fov / 2.0,
                            horizon_angles, bearings, args.visibility_tolerance, args.wikidata_prominence, args.offline)

    sun_now = sun_event = None
    if not args.no_sun:
        local = datetime.combine(date.fromisoformat(args.date), dtime.fromisoformat(args.time))
        sun_now, sun_event = sun_track(lat, lon, local, args.tz, heading, fov, horizon_angles, width)
    route_cross = None
    if not args.no_route:
        bearing = (heading + 8.0 if args.route_bearing is None else args.route_bearing) % 360.0
        if abs(angular_delta(bearing, heading)) < fov / 2.0 - 2.0:
            route_cross = route_crossing(dem, lat, lon, observer_elevation, args.eye_height,
                                         bearing, args.max_range * 1000.0)

    image, labelled, selected_peak = render(
        horizon_angles, layer_horizons, visible, heading, fov,
        width, args.max_labels, args.min_score, args.peak_step,
        sun_now, sun_event, route_cross
    )
    save_pair(image, path, args.scale)
    names = ", ".join(peak.name for peak in labelled) or "none"
    bands = " / ".join(f"{edge / 1000:.1f}" for edge in layer_edges)
    print(f"Adaptive layer edges: {bands} km")
    if selected_peak:
        print(f"Selected peak: {selected_peak.name} ({args.peak_step:+d} steps from heading)")
    if sun_now:
        print(f"Sun now: azimuth {sun_now[0]:.1f}, elevation {sun_now[1]:.1f}")
    if sun_event:
        print(f"Sun {sun_event[2]} at {sun_event[0]} (azimuth {sun_event[1]:.1f})")
    if route_cross:
        print(f"Route crosses the skyline at bearing {route_cross[0]:.1f}, {route_cross[1] / 1000:.1f} km out")
    print(f"Observer elevation {observer_elevation:.0f} m; {len(visible)} visible named peaks; labelled: {names}")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--preset", choices=PRESETS, default="gornergrat")
    result.add_argument("--lat", type=float, help="observer latitude; overrides the preset")
    result.add_argument("--lon", type=float, help="observer longitude; overrides the preset")
    result.add_argument("--heading", type=float, help="view direction in degrees clockwise from north")
    result.add_argument("--fov", type=float, default=120.0, help="horizontal field of view (default: 120)")
    result.add_argument("--eye-height", type=float, default=2.0, help="metres above DEM surface (default: 2)")
    result.add_argument("--max-range", type=float, default=100.0, help="ray and peak range in kilometres")
    result.add_argument("--layers", type=int, choices=LAYER_PALETTES, default=3,
                        help="terrain depth layers (default: 3; use 1 for a flat silhouette)")
    result.add_argument("--max-labels", type=int, default=5)
    result.add_argument("--peak-step", type=int, default=0,
                        help="preview Up/Down selection: negative is previous, positive is next in azimuth order")
    result.add_argument("--min-score", type=float, default=0.0, help="minimum elevation x isolation score")
    result.add_argument("--visibility-tolerance", type=float, default=0.15, help="peak/DEM skyline tolerance in degrees")
    result.add_argument("--wikidata-prominence", action="store_true", help="use cached P2660 prominence when available")
    result.add_argument("--date", default=date.today().isoformat(), help="local date for the sun, YYYY-MM-DD")
    result.add_argument("--time", default="17:30", help="local time of day for the sun, HH:MM")
    result.add_argument("--tz", type=float, default=2.0, help="local offset from UTC in hours (default: CEST)")
    result.add_argument("--no-sun", action="store_true", help="drop the sun disc and skyline-crossing time")
    result.add_argument("--route-bearing", type=float,
                        help="mock route bearing in degrees (default: heading + 8)")
    result.add_argument("--no-route", action="store_true", help="drop the mock route's skyline crossing")
    result.add_argument("--strip", action="store_true", help="also render a 720 px full-360-degree strip")
    result.add_argument("--offline", action="store_true", help="fail rather than download missing cache entries")
    result.add_argument("--scale", type=int, choices=range(2, 9), default=4, help="nearest-neighbour preview scale")
    result.add_argument("--out", type=Path, default=Path("out.png"))
    return result


def main() -> None:
    args = parser().parse_args()
    preset_lat, preset_lon, preset_heading, _ = PRESETS[args.preset]
    if (args.lat is None) != (args.lon is None):
        raise SystemExit("--lat and --lon must be supplied together")
    lat = preset_lat if args.lat is None else args.lat
    lon = preset_lon if args.lon is None else args.lon
    heading = (preset_heading if args.heading is None else args.heading) % 360.0
    if not 1.0 <= args.fov <= 360.0 or not 1.0 <= args.max_range <= 200.0:
        raise SystemExit("--fov must be 1..360 and --max-range must be 1..200 km")
    if args.max_labels < 0:
        raise SystemExit("--max-labels cannot be negative")

    render_view(args, lat, lon, heading, WIDTH, args.fov, args.out)
    if args.strip:
        strip_path = args.out.with_name(f"{args.out.stem}_strip.png")
        render_view(args, lat, lon, heading, STRIP_WIDTH, 360.0, strip_path)


if __name__ == "__main__":
    main()
