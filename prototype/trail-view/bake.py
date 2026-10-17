# /// script
# requires-python = ">=3.12"
# dependencies = ["rasterio==1.5.2", "pyproj==3.8.0", "numpy==2.5.3", "truststore==0.10.4"]
# ///
"""Bake one Trail View preset into ~/.cache/obc/trail-view/<preset>/.

    uv run prototype/trail-view/bake.py hahnen

Every raster is warped into one azimuthal-equidistant frame centred on the eye, so x and y are
metres east and north. Level k is 1024² px over ±1.25 km · 2^k.
"""

import argparse
import gzip
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import rasterio
import truststore
from pyproj import CRS, Transformer

# Certificates are checked against the system's trust store, as curl does: Spain's IGN serves a
# chain whose root Python's own bundle does not hold.
truststore.inject_into_ssl()

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path[:0] = [str(REPO / "host/obc-dem/reference"), str(REPO)]
from ingest.sources import SOURCES as DTM_SOURCES  # noqa: E402
from ingest.sources.base import http_download  # noqa: E402
from ingest.sources.es import ES  # noqa: E402
from ingest.sources.fr import FR, BilWmsSource  # noqa: E402
from ingest.sources.protocols import ArcGisSource  # noqa: E402
from ingest.sources.stac import stac_items  # noqa: E402
from ingest.sources.us import US  # noqa: E402
from tools.planner_snow import cdse_credentials  # noqa: E402

CACHE = Path.home() / ".cache/obc/trail-view"
SOURCES = CACHE / "_sources"

LEVELS = 9
SIZE = 1024
HALF0_M = 1250.0
# The national DTM covers k ≤ 3, ±10 km. The owner chose it after ±5 km put the Rigidalstock
# ridge, 4.7 km from Brunni, into the blend toward GLO-30.
DTM_LEVELS = 4
# The height above the ground the Peak View photographs were taken from. A preset can raise its
# eye with `eye_above_ground_m`, as a lookout's deck stands above the summit rocks.
PHONE_M = 1.6
NODATA = -32768.0

GLO30_LOCAL = Path.home() / ".cache/obcm/dem"
GLO30_S3 = "https://copernicus-dem-30m.s3.amazonaws.com"
GLO30_PX_M = 30.0
PHOTO_JSON = REPO / "fixtures/sources/peak-view/photos"
# 3DEP at 2 m: the registry's 1 m row fetches four times the pixels the levels can show.
US_2M = ArcGisSource("us", "United States", "3DEP 2 m", 2.0, "Public domain", "NAVD88", url=US.service)
# RGE ALTI at 2 m for the same reason: at 1 m a ±10 km window is 400 M pixels.
FR_2M = BilWmsSource("fr", "France", "RGE ALTI 2 m", 2.0, FR.licence, FR.vertical_datum, url=FR.service, layer=FR.layer)
# Services that answer one box at a time. request_boxes sizes a split at the box's middle
# latitude, so a row further from the pole can overrun a service's 2000 px cap; 3 km boxes stay
# far below it at these resolutions.
BOXED = {"us": US_2M, "fr": FR_2M, "es": ES}
BOX_M = 3000
# Public Overpass instances, tried in turn while one is busy.
OVERPASS = ("https://overpass-api.de/api/interpreter", "https://overpass.kumi.systems/api/interpreter")
USER_AGENT = "OpenBikeComputer trail view prototype (https://github.com/timohueser/OpenBikeComputer)"

WORLDCOVER = "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_{}_Map.tif"
WORLDCOVER_PX_M = 10.0
# WorldCover classes in the order of the page's palette ramps. 255 is unknown.
CLASSES = (10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 100)
WATER = CLASSES.index(80)
UNKNOWN = 255

S2_STAC = "https://stac.dataspace.copernicus.eu/v1/collections/sentinel-2-global-mosaics/items"
S2_YEAR = 2025
S2_BANDS = ("B02", "B03", "B04", "B08", "observations")
# Colour reaches k ≤ 6, ±80 km; beyond it, haze dominates and the class palette is enough.
S2_LEVELS = 7
# A pixel with fewer valid looks in its quarter falls back to the palette.
MIN_OBSERVATIONS = 3
# Below this visible reflectance a winter pixel is mostly terrain shadow, so its chroma comes
# from summer.
DARK = 0.025

# The near field: road surfaces on k < ROAD_LEVELS (±5 km), buildings within BUILDING_HALF_M,
# instanced trees within TREE_RADIUS_M on a jittered grid of TREE_SPACING_M, about 1.5 trees per
# 10 m WorldCover pixel.
ROAD_LEVELS = 3
BUILDING_HALF_M = 2500.0
TREE_RADIUS_M = 1000.0
TREE_SPACING_M = 8.0
TREE = CLASSES.index(10)
# Road surfaces in the order the page's shader paints them (code = index + 1, 0 = no road), with
# the half-width each is painted at. The field holds the distance to the nearest road EDGE, which
# stays continuous where two roads of different widths meet; a byte is 0.1 m from -5 m to 20.5 m.
ROAD_WIDTHS = {"main": 4.5, "road": 3.0, "gravel": 1.75, "dirt": 1.5, "grass": 1.25, "path": 0.6}
ROAD_REACH_M = 20.5
LEAF = {"needleleaved": 1, "broadleaved": 2, "mixed": 3}
# Tree heights where no DTM gives a canopy: hedgerow and garden trees.
NO_CANOPY_M = (5.0, 9.0)
# OSM water on k < WATER_LEVELS (±10 km), as coverage from a 4× finer grid: rivers and canals
# narrower than a WorldCover pixel, and lakes. Full widths for a waterway without a width tag.
# Beyond, WorldCover's water class carries what is wide enough to see.
WATER_LEVELS = 4
WATERWAY_WIDTHS = {"river": 12.0, "canal": 8.0, "stream": 2.0}
SUPERSAMPLE = 4
# The water levels' second byte: metres from water to the nearest land as sqrt(d / SHORE_REACH_M),
# so the shallows near a shore keep their detail and open water reaches the deep colour.
SHORE_REACH_M = 600.0
# Where GLO-30 is the only elevation, a cliff coast is a 30 m slope into the sea. The sea beside land
# stands at sea level, so the coast drops where WorldCover puts the water's edge; land this close
# to it rises toward the highest ground within reach, by at most CLIFF_RAISE_M, where that is a
# cliff's height. A larger raise lifts the edge to ground that only rises further inland.
CLIFF_REACH_M = 15.0
CLIFF_RAISE_M = 6.0
CLIFF_MIN_M = 12.0
SEA_LEVEL_M = 3.0

# Geology for rock colour on k < ROCK_LEVELS: IGME's continuous 1:50 000 map, layer 1 is the
# units' fill. Its colours are cartographic, so each unit's own lithology text picks the colour.
GEOLOGY = {"es": ("https://mapas.igme.es/gis/services/Cartografia_Geologica/IGME_Geode_50/MapServer/WMSServer", "1",
                  "GEODE 1:50 000 © IGME")}
ROCK_LEVELS = 3
# Words in a unit's description and the rock colour they mean, first match first. Variegated rock
# is drawn in red, green and white bands.
LITHOLOGY = [
    ("abigarrad", "#a8604e", True), ("blanc", "#ddd6c4", False), ("yeso", "#e2dccd", False),
    ("roj", "#a9583f", False), ("salm", "#c98c6e", False), ("verde", "#8f9884", False),
    ("calcaren", "#cdb88e", False), ("caliza", "#c4bdb0", False), ("dolom", "#b2a894", False),
    ("arenisca", "#c3a06c", False), ("conglomerad", "#a99878", False), ("marga", "#d2c4a4", False),
    ("arcilla", "#b58a64", False), ("pizarra", "#6e6a66", False), ("granit", "#b5aba0", False),
    ("cuarcita", "#bfb9ae", False), ("esquisto", "#7b766c", False),
    ("glacis", "#c2ad88", False), ("coluvi", "#c2ad88", False), ("aluvi", "#c2ad88", False),
]
ROCK = "#b0a796"
# Fine habitats for the ground mosaic on k < HABITAT_LEVELS, in one vocabulary whatever the source;
# the page's region kit paints each. The source is the best open land cover whose box holds the eye.
HABITATS = ("none", "rock", "heath", "bog", "bracken", "rough", "pasture", "scrub", "hedge", "sand", "saltmarsh", "built",
            "water")
HABITAT_LEVELS = 3
# Ireland's National Land Cover Map 2018, mapped from 1 m imagery to 0.05 ha: (west, south, east, north),
# the feature service, and its LEVEL_2_VALUE words for each habitat, the first match winning.
NLCM = ((-10.7, 51.35, -5.9, 55.45),
        "https://services-eu1.arcgis.com/FH5XCsx8rYXqnjF5/arcgis/rest/services/MapGenieNationalLandCover2018ITM/FeatureServer/0",
        "National Land Cover Map 2018 © Tailte Éireann and EPA, CC BY 4.0")
NLCM_WORDS = [("exposed rock", "rock"), ("heath", "heath"), ("bog", "bog"), ("peat", "bog"), ("fen", "bog"),
              ("swamp", "bog"), ("bracken", "bracken"), ("wet grassland", "rough"), ("dry grassland", "rough"),
              ("improved grassland", "pasture"), ("amenity grassland", "pasture"), ("scrub", "scrub"),
              ("transitional forest", "scrub"), ("hedgerow", "hedge"), ("treeline", "hedge"), ("sand", "sand"),
              ("coastal sediments", "sand"), ("mudflat", "sand"), ("saltmarsh", "saltmarsh"), ("artificial", "built"),
              ("buildings", "built"), ("ways", "built"), ("bare soil", "built"), ("water", "water"),
              ("lakes", "water"), ("rivers", "water")]
# Burn order: broad covers first, so the small and linear ones they hold stay on top.
HABITAT_ORDER = ("pasture", "rough", "heath", "bog", "bracken", "saltmarsh", "sand", "scrub", "water", "built", "rock", "hedge")

# OSM features near the eye: walls and hedges within WALL_HALF_M, cut into pieces of at most
# WALL_PIECE_M so each stands on the ground; orchard trees in rows ORCHARD_SPACING_M apart.
WALL_HALF_M = 1500.0
WALL_PIECE_M = 8.0
ORCHARD_SPACING_M = 7.0

GDAL_OPTIONS = {"GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR", "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif,.vrt",
                "GDAL_HTTP_MAX_RETRY": "4", "GDAL_HTTP_RETRY_DELAY": "5"}

CREDITS = {
    "glo30": "Copernicus DEM GLO-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the European Union and ESA",
    "ch": "swissALTI3D © swisstopo",
    "de-bw": "DGM1 © LGL Baden-Württemberg, dl-de/by-2-0",
    "us": "USGS 3D Elevation Program, public domain",
    "fr": "RGE ALTI © IGN, Licence Ouverte",
    "es": "MDT05 © Instituto Geográfico Nacional, CC BY 4.0",
    "worldcover": "ESA WorldCover 10 m 2021 v200 © ESA WorldCover project, CC BY 4.0",
    "s2": f"Contains modified Copernicus Sentinel data {S2_YEAR} (Sentinel-2 quarterly mosaics)",
    "osm": "© OpenStreetMap contributors (ODbL)",
    "photo": "Photograph © Timo Hüser, CC BY 4.0",
}


def fixture_photo(name: str) -> Path:
    """The photograph of the `peak-view-photos` fixture package, as `obc_fixtures` finds it."""
    if root := os.environ.get("OBC_FIXTURE_CACHE"):
        base = Path(root) / "by-id"
    elif root := os.environ.get("XDG_CACHE_HOME"):
        base = Path(root) / "openbikecomputer/fixtures/by-id"
    else:
        base = Path.home() / ".cache/openbikecomputer/fixtures/by-id"
    return base / "peak-view-photos/photos" / f"{name}.jpg"


def load_preset(preset_id: str) -> dict:
    presets = {p["id"]: p for p in json.loads((HERE / "presets.json").read_text())}
    if preset_id not in presets:
        sys.exit(f"unknown preset {preset_id!r}; presets.json has {', '.join(presets)}")
    preset = dict(presets[preset_id])
    photo = preset.get("photo")
    if isinstance(photo, str):
        # A Peak View fixture: position, ground and camera come with the photograph.
        fixture = json.loads((PHOTO_JSON / f"{photo}.json").read_text())
        preset.update(lat=fixture["lat"], lon=fixture["lon"], ground_m=fixture["ground_m"])
        preset["photo"] = {key: fixture[key] for key in
                           ("heading_deg", "pitch_deg", "lens_mm", "columns", "rms_limit_deg", "subject")}
        preset["photo"]["credit"] = CREDITS["photo"]
        preset["photo_source"] = fixture_photo(photo)
    elif photo:
        # A third-party photograph, kept in the cache only; its camera was aligned in the page.
        suffix = Path(urllib.parse.urlparse(photo["url"]).path).suffix.lower()
        preset["photo_source"] = http_download(photo["url"], SOURCES / "photos" / f"{preset_id}{suffix}",
                                               headers={"User-Agent": USER_AGENT})
        preset["photo"] = {key: value for key, value in photo.items() if key != "url"}
    if preset.get("photo"):
        preset["photo"]["file"] = "photo.jpg"
        preset.update(heading=preset["photo"]["heading_deg"], pitch=preset["photo"]["pitch_deg"])
    return preset


def glo30_tiles(bbox, download: bool) -> list[str]:
    """Every GLO-30 tile the box touches: a local copy if there is one, else the public COG,
    downloaded first when the warp reads it at full resolution."""
    listing = SOURCES / "glo30-tiles.txt"
    if not listing.exists():
        listing.parent.mkdir(parents=True, exist_ok=True)
        listing.write_bytes(urllib.request.urlopen(f"{GLO30_S3}/tileList.txt", timeout=120).read())
    published = set(listing.read_text().split())
    west, south, east, north = bbox
    tiles = []
    for lat in range(int(np.floor(south)), int(np.floor(north)) + 1):
        for lon in range(int(np.floor(west)), int(np.floor(east)) + 1):
            name = (f"Copernicus_DSM_COG_10_{'N' if lat >= 0 else 'S'}{abs(lat):02d}_00_"
                    f"{'E' if lon >= 0 else 'W'}{abs(lon):03d}_00_DEM")
            local = GLO30_LOCAL / f"{name}.tif"
            cached = SOURCES / "glo30" / f"{name}.tif"
            if local.exists() or cached.exists():
                tiles.append(str(local if local.exists() else cached))
            elif name in published and download:
                tiles.append(str(http_download(f"{GLO30_S3}/{name}/{name}.tif", cached)))
            elif name in published:
                tiles.append(f"/vsicurl/{GLO30_S3}/{name}/{name}.tif")
    return tiles


def warp(inputs, frame: str, half: float, resampling: str, overviews: bool, work: Path,
         threads: bool = True) -> np.ndarray:
    """Every band of the inputs on level grid `half`, as float32 with NODATA."""
    out = work / "warp.tif"
    cmd = ["gdalwarp", "-q", "-overwrite", "-t_srs", frame, "-te", str(-half), str(-half), str(half), str(half),
           "-ts", str(SIZE), str(SIZE), "-r", resampling, "-ot", "Float32", "-dstnodata", str(NODATA)]
    if threads:
        cmd += ["-multi", "-wo", "NUM_THREADS=ALL_CPUS"]
    if not overviews:
        cmd += ["-ovr", "NONE"]
    subprocess.run(cmd + [str(p) for p in inputs] + [str(out)], check=True, env={**os.environ, **GDAL_OPTIONS})
    with rasterio.open(out) as src:
        return src.read()


def glo30_level(k: int, frame: str, bbox, work: Path) -> np.ndarray:
    half = HALF0_M * 2**k
    px = 2 * half / SIZE
    # Upsampling 30 m cells needs interpolation; downsampling keeps each summit with max.
    resampling = "bilinear" if px < GLO30_PX_M else "max"
    # COG overviews are averaged, which flattens summits; past k = 5 (156 m px) that costs
    # under 0.03° and saves reading full-resolution remote tiles.
    heights = warp(glo30_tiles(bbox, download=k <= 5), frame, half, resampling, overviews=k > 5, work=work)[0]
    # Missing tiles are sea.
    heights[heights == NODATA] = 0.0
    return heights


def dtm_rasters(key: str, bbox) -> list[Path]:
    if key == "de-bw":
        return bw_rasters(bbox)
    if key in BOXED:
        west, south, east, north = bbox
        rows = math.ceil((north - south) * 111_320 / BOX_M)
        cols = math.ceil((east - west) * 111_320 * math.cos(math.radians(south)) / BOX_M)
        boxes = [(west + (east - west) * j / cols, south + (north - south) * i / rows,
                  west + (east - west) * (j + 1) / cols, south + (north - south) * (i + 1) / rows)
                 for i in range(rows) for j in range(cols)]
        return [path for box in boxes for path in BOXED[key].fetch(box, SOURCES / key)]
    return DTM_SOURCES[key].fetch(bbox, SOURCES / key)


def bw_rasters(bbox) -> list[Path]:
    """Baden-Württemberg's DGM1 as compressed GeoTIFFs.

    A 2 km zip unpacks to 116 MB of XYZ text, so each zip is converted and its text deleted
    before the next. The heights are metres, though the registry row says centimetres.
    """
    out = SOURCES / "de-bw"
    out.mkdir(parents=True, exist_ok=True)

    def one(item) -> list[Path]:
        name, url = item
        done = out / f"{name}.tifs"
        if not done.exists():
            converted = []
            with tempfile.TemporaryDirectory(dir=out) as tmp:
                # A square outside the state is not published.
                archive = http_download(url, Path(tmp) / name, optional=True)
                members = zipfile.ZipFile(archive).namelist() if archive else []
                for member in (m for m in members if m.endswith(".xyz")):
                    xyz = Path(zipfile.ZipFile(archive).extract(member, tmp))
                    tif = out / f"{xyz.stem}.tif"
                    subprocess.run(["gdal_translate", "-q", "-a_srs", "EPSG:25832", "-co", "COMPRESS=DEFLATE",
                                    "-co", "PREDICTOR=3", "-co", "TILED=YES", str(xyz), str(tif)], check=True)
                    xyz.unlink()
                    converted.append(tif.name)
            done.write_text("\n".join(converted))
        return [out / tif for tif in done.read_text().split()]

    with ThreadPoolExecutor(max_workers=4) as pool:
        return [tif for tifs in pool.map(one, DTM_SOURCES["de-bw"].files(bbox)) for tif in tifs]


def worldcover_tiles(bbox) -> list[str]:
    """Every published WorldCover tile the box touches. A tile wholly over the sea does not exist."""
    known_path = SOURCES / "worldcover-tiles.json"
    known = json.loads(known_path.read_text()) if known_path.exists() else {}
    west, south, east, north = bbox
    tiles = []
    for lat in range(math.floor(south / 3) * 3, math.floor(north / 3) * 3 + 1, 3):
        for lon in range(math.floor(west / 3) * 3, math.floor(east / 3) * 3 + 1, 3):
            url = WORLDCOVER.format(f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}")
            if url not in known:
                try:
                    urllib.request.urlopen(urllib.request.Request(url, method="HEAD"), timeout=60)
                    known[url] = True
                except urllib.error.HTTPError:
                    known[url] = False
            if known[url]:
                tiles.append(f"/vsicurl/{url}")
    known_path.parent.mkdir(parents=True, exist_ok=True)
    known_path.write_text(json.dumps(known, indent=1))
    return tiles


def class_level(k: int, frame: str, bbox, heights: np.ndarray, work: Path) -> np.ndarray:
    """RG8: the palette index of the WorldCover class, and 255 where it is water."""
    half = HALF0_M * 2**k
    resampling = "near" if 2 * half / SIZE <= WORLDCOVER_PX_M else "mode"
    raw = warp(worldcover_tiles(bbox), frame, half, resampling, overviews=True, work=work)[0]
    lookup = np.full(256, UNKNOWN, np.uint8)
    lookup[list(CLASSES)] = np.arange(len(CLASSES))
    index = lookup[np.where(raw == NODATA, 0, raw).astype(np.uint8)]
    # No tile is published over open sea.
    index[(index == UNKNOWN) & (heights <= 0.5)] = WATER
    index = round_shores(index, half)
    return np.stack([index, np.where(index == WATER, 255, 0).astype(np.uint8)], axis=-1)


def round_shores(index: np.ndarray, half: float) -> np.ndarray:
    """Finer than WorldCover's pixels, its shores are stair-stepped: water by the majority within
    half a pixel instead, and land that loses its water takes the nearest land class."""
    r = round(WORLDCOVER_PX_M / 2 / (2 * half / SIZE))
    if not r:
        return index
    index = index.copy()
    water = box_mean((index == WATER).astype(np.float32), r) > 0.5
    lost = (index == WATER) & ~water
    index[water] = WATER
    index[lost] = UNKNOWN
    for _ in range(2 * r + 1):
        for axis, step in ((0, 1), (0, -1), (1, 1), (1, -1)):
            shifted = np.roll(index, step, axis=axis)
            fill = lost & (index == UNKNOWN) & (shifted != UNKNOWN) & (shifted != WATER)
            index[fill] = shifted[fill]
    index[lost & (index == UNKNOWN)] = CLASSES.index(30)
    return index


def s2_items(bbox, work: Path) -> dict[int, list[Path]]:
    """Per quarter, one five-band VRT per mosaic tile the box touches, read over CDSE S3."""
    url = (f"{S2_STAC}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}"
           f"&datetime={S2_YEAR}-01-01T00:00:00Z/{S2_YEAR}-12-31T23:59:59Z&limit=100")
    quarters = {q: [] for q in range(1, 5)}
    for item in stac_items(url):
        quarter = (int(item["properties"]["start_datetime"][5:7]) - 1) // 3 + 1
        vrt = work / f"{item['id']}.vrt"
        bands = [item["assets"][band]["href"].replace("s3://", "/vsis3/") for band in S2_BANDS]
        subprocess.run(["gdalbuildvrt", "-q", "-separate", str(vrt), *bands], check=True,
                       env={**os.environ, **GDAL_OPTIONS})
        quarters[quarter].append(vrt)
    return quarters


def s2_level(k: int, frame: str, quarters: dict[int, list[Path]], work: Path) -> dict[int, np.ndarray]:
    """RGBA8 per quarter: visible reflectance as refl^(1/2.2), and NDVI as 1–255, 0 where looks
    are few. Winter pixels in terrain shadow take their chroma from summer."""
    half = HALF0_M * 2**k
    resampling = "bilinear" if 2 * half / SIZE < 10 else "average"
    seen = {}
    for q, vrts in quarters.items():
        # CDSE allows 4 connections per user, so these warps read on one thread.
        b02, b03, b04, b08, looks = warp(vrts, frame, half, resampling, overviews=True, work=work, threads=False)
        valid = (looks >= MIN_OBSERVATIONS) & (b04 != NODATA)
        visible = np.clip(np.stack([b04, b03, b02]) / 10000.0, 0.0, 1.0)
        ndvi = np.clip((b08 - b04) / np.maximum(b08 + b04, 1.0), -1.0, 1.0)
        seen[q] = (visible, ndvi, valid)
    summer = seen[3]
    for q in (1, 4):
        visible, ndvi, valid = seen[q]
        luma = visible.mean(axis=0)
        shaded_snow = (visible[2] > 1.15 * visible[0]) & (ndvi < 0.1)
        dark = valid & summer[2] & (luma < DARK) & ~shaded_snow
        scale = luma / np.maximum(summer[0].mean(axis=0), 1e-4)
        visible[:, dark] = (summer[0] * scale)[:, dark]
    texels = {}
    for q, (visible, ndvi, valid) in seen.items():
        rgb = np.round(255.0 * visible ** (1 / 2.2))
        a = np.where(valid, 1 + np.round((ndvi + 1) / 2 * 254), 0)
        texels[q] = np.stack([*rgb, a], axis=-1).astype(np.uint8)
    return texels


def road_surface(tags: dict) -> str | None:
    """Which painted surface a highway gets, or None for one that is not on the ground."""
    highway, surface, grade = tags["highway"], tags.get("surface", ""), tags.get("tracktype", "")
    if tags.get("tunnel") in ("yes", "building_passage") or highway in ("proposed", "construction", "platform"):
        return None
    if surface in ("asphalt", "paved", "concrete", "paving_stones", "sett", "chipseal"):
        return "main" if highway in ("motorway", "trunk", "primary", "secondary") else "road"
    if surface in ("gravel", "compacted", "fine_gravel", "pebblestone"):
        return "gravel"
    if surface in ("dirt", "ground", "earth", "mud", "unpaved", "sand"):
        return "dirt"
    if surface in ("grass", "grass_paver"):
        return "grass"
    if highway in ("path", "footway", "bridleway", "steps", "pedestrian", "via_ferrata"):
        return "path"
    if highway == "track":
        return {"grade1": "road", "grade2": "gravel", "grade3": "dirt"}.get(grade, "grass")
    if highway in ("motorway", "trunk", "primary", "secondary", "motorway_link", "trunk_link", "primary_link"):
        return "main"
    return "road"


def edge_distance(lines: list[tuple[np.ndarray, float]], half: float, size: int, reach: float):
    """Over a size² grid on ±half: the distance to the nearest edge of the lines, each drawn at
    its half-width, negative inside and at most `reach`; and that line's index + 1, 0 for none."""
    px = 2 * half / size
    distance = np.full((size, size), reach, np.float32)
    nearest = np.zeros((size, size), np.int32)
    for n, (xy, width) in enumerate(lines, 1):
        for (ax, ay), (bx, by) in zip(xy[:-1], xy[1:]):
            i0 = max(int((min(ax, bx) - width - reach + half) / px), 0)
            i1 = min(int((max(ax, bx) + width + reach + half) / px) + 1, size)
            j0 = max(int((half - max(ay, by) - width - reach) / px), 0)
            j1 = min(int((half - min(ay, by) + width + reach) / px) + 1, size)
            if i0 >= i1 or j0 >= j1:
                continue
            x, y = np.meshgrid(-half + (np.arange(i0, i1) + 0.5) * px, half - (np.arange(j0, j1) + 0.5) * px)
            dx, dy = bx - ax, by - ay
            t = np.clip(((x - ax) * dx + (y - ay) * dy) / max(dx * dx + dy * dy, 1e-9), 0, 1)
            d = np.hypot(x - ax - t * dx, y - ay - t * dy) - width
            closer = d < distance[j0:j1, i0:i1]
            distance[j0:j1, i0:i1][closer] = d[closer]
            nearest[j0:j1, i0:i1][closer] = n
    return distance, nearest


def road_field(roads: list[tuple[np.ndarray, int]], half: float) -> np.ndarray:
    """RG8: (distance to the nearest road edge + 5 m) in 0.1 m steps, negative inside a road, and
    that road's surface code."""
    widths = list(ROAD_WIDTHS.values())
    distance, nearest = edge_distance([(xy, widths[code - 1]) for xy, code in roads], half, SIZE, ROAD_REACH_M)
    surface = np.array([0] + [code for _, code in roads], np.uint8)[nearest]
    return np.stack([np.round((distance + 5) * 10).astype(np.uint8), surface], axis=-1)


def water_field(rivers, streams, lakes, classes: np.ndarray, half: float, work: Path) -> np.ndarray:
    """RG8: the share of each pixel under OSM water, 0–255, and its shore distance (SHORE_REACH_M).
    Streams count only off tree pixels, because the canopy hides them."""
    from rasterio.features import rasterize
    from rasterio.transform import from_origin

    fine = SIZE * SUPERSAMPLE
    reach = 2 * half / fine

    def coverage(wet: np.ndarray) -> np.ndarray:
        return wet.reshape(SIZE, SUPERSAMPLE, SIZE, SUPERSAMPLE).mean(axis=(1, 3))

    wet = edge_distance(rivers, half, fine, reach)[0] < 0
    if lakes:
        shapes = [({"type": "Polygon", "coordinates": [ring.tolist()]}, 1) for ring in lakes]
        wet |= rasterize(shapes, out_shape=(fine, fine), transform=from_origin(-half, half, reach, reach),
                         dtype=np.uint8).astype(bool)
    water = coverage(wet)
    if streams:
        water = np.maximum(water, coverage(edge_distance(streams, half, fine, reach)[0] < 0) * (classes[..., 0] != TREE))
    shore = shore_distance((classes[..., 1] == 255) | (water >= 0.5), half, work)
    return np.stack([np.round(water * 255), np.round(np.sqrt(shore / SHORE_REACH_M) * 255)], axis=-1).astype(np.uint8)


def shore_distance(water: np.ndarray, half: float, work: Path) -> np.ndarray:
    """Metres from each water texel to the nearest land texel, at most SHORE_REACH_M; 0 on land."""
    src, dst = work / "land.tif", work / "shore.tif"
    with rasterio.open(src, "w", driver="GTiff", height=SIZE, width=SIZE, count=1, dtype="uint8",
                       transform=rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE)) as f:
        f.write((~water).astype(np.uint8)[None])
    subprocess.run(["gdal_proximity", "-q", str(src), str(dst), "-values", "1", "-distunits", "GEO", "-ot", "Float32",
                    "-maxdist", str(SHORE_REACH_M), "-nodata", str(SHORE_REACH_M)], check=True)
    with rasterio.open(dst) as f:
        return np.minimum(f.read(1), SHORE_REACH_M) * water


def grow(a: np.ndarray, op) -> np.ndarray:
    """One 3×3 step of a filter: np.maximum for a max filter, np.logical_or for a dilation."""
    out = a.copy()
    out[1:] = op(out[1:], a[:-1])
    out[:-1] = op(out[:-1], a[1:])
    rows = out.copy()
    out[:, 1:] = op(out[:, 1:], rows[:, :-1])
    out[:, :-1] = op(out[:, :-1], rows[:, 1:])
    return out


def sharpen_coast(heights: np.ndarray, classes: np.ndarray, half: float) -> np.ndarray:
    """GLO-30 heights with cliff coasts made sheer (see CLIFF_REACH_M)."""
    water = classes[..., 1] == 255
    sea = water & (heights <= SEA_LEVEL_M)
    if not sea.any():
        return heights
    # GLO-30 smears a cliff over the sea beside it too: water joined to the sea within reach of
    # the shore is sea, however high the DEM puts it.
    reach = max(1, round(CLIFF_REACH_M * 4 / (2 * half / SIZE)))
    for _ in range(reach):
        sea |= grow(sea, np.logical_or) & water
    top, near = heights, sea
    for _ in range(max(1, round(CLIFF_REACH_M / (2 * half / SIZE)))):
        top, near = grow(top, np.maximum), grow(near, np.logical_or)
    rim = ~water & near & (top >= CLIFF_MIN_M)
    out = heights.copy()
    out[rim] = np.maximum(heights[rim], np.minimum(top[rim], heights[rim] + CLIFF_RAISE_M))
    out[sea] = np.minimum(heights[sea], 0.0)
    return out


def wms_map(service: str, layer: str, fmt: str, bbox, px: int, path: Path, info: tuple[int, int] | None = None) -> Path:
    """One WMS 1.3.0 map of a WGS84 box (latitude first, as 1.3.0 states EPSG:4326), or the
    feature info at a pixel of that map."""
    west, south, east, north = bbox
    params = {"SERVICE": "WMS", "VERSION": "1.3.0", "REQUEST": "GetMap", "LAYERS": layer, "STYLES": "",
              "CRS": "EPSG:4326", "BBOX": f"{south},{west},{north},{east}", "WIDTH": px, "HEIGHT": px, "FORMAT": fmt}
    if info:
        params.update(REQUEST="GetFeatureInfo", QUERY_LAYERS=layer, I=info[0], J=info[1], INFO_FORMAT="text/xml")
    return http_download(f"{service}?{urllib.parse.urlencode(params)}", path)


def onto_level(bands: np.ndarray, bbox, k: int, frame: str, size: int, resampling: str, work: Path) -> np.ndarray:
    """uint8 bands over a WGS84 box, warped onto level k's grid at size² texels."""
    half = HALF0_M * 2**k
    src, dst = work / "wms.tif", work / "wms-warp.tif"
    profile = {"driver": "GTiff", "height": bands.shape[1], "width": bands.shape[2], "count": bands.shape[0],
               "dtype": "uint8", "crs": "EPSG:4326", "transform": rasterio.transform.from_bounds(*bbox, bands.shape[2], bands.shape[1])}
    with rasterio.open(src, "w", **profile) as f:
        f.write(bands)
    # A fourth band is data here, not alpha.
    subprocess.run(["gdalwarp", "-q", "-overwrite", "-nosrcalpha", "-t_srs", frame, "-te", str(-half), str(-half), str(half),
                    str(half), "-ts", str(size), str(size), "-r", resampling, str(src), str(dst)], check=True)
    with rasterio.open(dst) as f:
        return f.read()


def lithology(text: str) -> tuple[str, bool]:
    lower = text.lower()
    return next(((colour, banded) for word, colour, banded in LITHOLOGY if word in lower), (ROCK, False))


def rock_level(key: str, k: int, frame: str, bbox, work: Path, units: dict) -> np.ndarray:
    """RGBA8 on level k: the rock colour of the geological unit, and 255 where it is variegated.
    The map's fill colours stand for units; hatching and antialiased edges are minor colours, which
    take the nearest major one. `units` caches each fill colour's description across levels."""
    service, layer, _ = GEOLOGY[key]
    px = 2048
    with rasterio.open(wms_map(service, layer, "image/png", bbox, px, work / f"geology-{k}.png")) as f:
        rgb = f.read()[:3].astype(np.int64)
    code = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2]
    values, counts = np.unique(code, return_counts=True)
    # Black is the map's boundary lines and labels.
    majors = values[(counts >= 0.002 * code.size) & (values != 0)]
    label = np.full(code.shape, -1)
    for i, value in enumerate(majors):
        label[code == value] = i
    for _ in range(16):
        for axis, step in ((0, 1), (0, -1), (1, 1), (1, -1)):
            shifted = np.roll(label, step, axis=axis)
            fill = (label < 0) & (shifted >= 0)
            label[fill] = shifted[fill]
    label[label < 0] = int(np.argmax([np.sum(label == i) for i in range(len(majors))]))
    colours = np.zeros((len(majors), 4), np.uint8)
    for i, value in enumerate(majors):
        if value not in units:
            inside = label == i
            for axis, step in ((0, 4), (0, -4), (1, 4), (1, -4)):
                inside &= np.roll(label == i, step, axis=axis)
            js, is_ = np.nonzero(inside if inside.any() else label == i)
            j, x = js[len(js) // 2], is_[len(is_) // 2]
            body = wms_map(service, layer, "image/png", bbox, px, work / f"info-{k}-{value}.xml", info=(int(x), int(j))).read_text()
            match = re.search(r'DescripciónUnidadGeológica="([^"]*)"', body)
            units[value] = match.group(1) if match else ""
        colour, banded = lithology(units[value])
        colours[i] = [int(colour[1:3], 16), int(colour[3:5], 16), int(colour[5:7], 16), 255 if banded else 0]
    return np.moveaxis(onto_level(np.moveaxis(colours[label], -1, 0), bbox, k, frame, SIZE, "near", work), 0, -1)


def box_mean(a: np.ndarray, r: int) -> np.ndarray:
    """The mean over a (2r + 1)² box, edges repeated."""
    n = 2 * r + 1
    s = np.pad(np.pad(a, r, mode="edge").cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    return (s[n:, n:] - s[:-n, n:] - s[n:, :-n] + s[:-n, :-n]) / (n * n)


def canopy_of(glo30: np.ndarray, dtm: np.ndarray, classes: np.ndarray, half: float) -> np.ndarray:
    """GLO-30, a surface model, minus the DTM on tree pixels, clamped to 0–60 m. The DTM is
    averaged over GLO-30's own 30 m first, so terrain detail does not read as canopy."""
    canopy = np.clip(glo30 - box_mean(dtm, round(15 / (2 * half / SIZE))), 0, 60)
    canopy[classes[..., 0] != TREE] = 0
    return canopy


def sample(level: np.ndarray, half: float, x, y):
    """Nearest texel of a level at metres east and north."""
    px = 2 * half / SIZE
    i = np.clip(((np.asarray(x) + half) / px).astype(int), 0, SIZE - 1)
    j = np.clip(((half - np.asarray(y)) / px).astype(int), 0, SIZE - 1)
    return level[j, i]


def scatter_trees(classes0, roads0, water0, canopy0, leaf0, default_leaf: str) -> np.ndarray:
    """float32 x, y, height, type (0 conifer, 1 broadleaf) for every tree within TREE_RADIUS_M:
    jittered on tree pixels, outside the road buffers and water, as tall as the canopy says.
    GLO-30 alone cannot measure a canopy, so without a DTM trees stand NO_CANOPY_M tall."""
    rng = np.random.default_rng(1)
    grid = np.arange(-TREE_RADIUS_M, TREE_RADIUS_M, TREE_SPACING_M) + TREE_SPACING_M / 2
    x, y = (axis.ravel() + rng.uniform(-0.4, 0.4, axis.size) * TREE_SPACING_M for axis in np.meshgrid(grid, grid))
    half = HALF0_M
    edge = sample(roads0, half, x, y)[:, 0] / 10 - 5
    canopy = sample(canopy0, half, x, y) if canopy0 is not None else rng.uniform(*NO_CANOPY_M, x.size)
    keep = ((np.hypot(x, y) <= TREE_RADIUS_M) & (sample(classes0, half, x, y)[:, 0] == TREE)
            & (edge > 1.5) & (sample(water0, half, x, y) < 64) & (canopy > 3))
    x, y, canopy = x[keep], y[keep], canopy[keep]
    leaf = sample(leaf0, half, x, y)
    leaf[leaf == 0] = LEAF[default_leaf]
    broad = np.where(leaf == 3, rng.random(leaf.size) < 0.5, leaf == 2)
    height = np.clip(canopy, NO_CANOPY_M[0], 45) * rng.uniform(0.8, 1.15, canopy.size)
    return np.stack([x, y, height, broad.astype(np.float32)], axis=-1).astype(np.float32)


def nlcm(preset_id: str, lat: float, lon: float, to_wgs84) -> list[tuple[list, str]]:
    """The land cover map's polygons over the habitat levels as WGS84 rings with their habitat,
    cached like `osm_ways`. The service pages 2000 polygons at a time, slowly, so pages run three
    at once."""
    cached = SOURCES / "nlcm" / f"{preset_id}_{lat:.5f}_{lon:.5f}.json"
    if not cached.exists():
        half = HALF0_M * 2 ** (HABITAT_LEVELS - 1)
        w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
        query = {"where": "1=1", "geometry": f"{w},{s},{e},{n}", "geometryType": "esriGeometryEnvelope", "inSR": 4326,
                 "spatialRel": "esriSpatialRelIntersects", "outFields": "LEVEL_2_VALUE", "outSR": 4326, "f": "json"}
        get = lambda **extra: json.loads(urllib.request.urlopen(
            f"{NLCM[1]}/query?{urllib.parse.urlencode({**query, **extra})}", timeout=600).read())
        count = get(returnCountOnly="true", returnGeometry="false")["count"]
        with ThreadPoolExecutor(3) as pool:
            pages = pool.map(lambda offset: get(returnGeometry="true", resultOffset=offset, resultRecordCount=2000),
                             range(0, count, 2000))
            rows = [[f["geometry"]["rings"], f["attributes"]["LEVEL_2_VALUE"]] for page in pages for f in page["features"]
                    if f.get("geometry")]
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_text(json.dumps(rows))
    habitat = lambda value: next((h for word, h in NLCM_WORDS if word in (value or "").lower()), "none")
    return [(rings, habitat(value)) for rings, value in json.loads(cached.read_text())]


def habitat_level(polygons: list[tuple[list, str]], to_frame, half: float) -> np.ndarray:
    """R8 on level k: the index into HABITATS of each texel, 0 where the map has none."""
    from rasterio.features import rasterize

    def frame(rings):
        return [np.array(to_frame.transform(*np.array(ring).T)).T.tolist() for ring in rings]

    shapes = [({"type": "Polygon", "coordinates": frame(rings)}, HABITATS.index(h))
              for h in HABITAT_ORDER for rings, kind in polygons if kind == h]
    if not shapes:
        return np.zeros((SIZE, SIZE), np.uint8)
    return rasterize(shapes, out_shape=(SIZE, SIZE), dtype=np.uint8,
                     transform=rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE))


def orchard_trees(orchards: list[np.ndarray]) -> np.ndarray:
    """float32 x, y, height, type 2 for orchard trees within TREE_RADIUS_M, in rows along each
    orchard's longest edge."""
    rng = np.random.default_rng(2)
    rows = []
    for ring in orchards:
        if np.hypot(*ring.T).min() > TREE_RADIUS_M:
            continue
        edges = np.diff(ring, axis=0)
        u = edges[np.argmax(np.hypot(*edges.T))]
        u = u / np.hypot(*u)
        v = np.array([-u[1], u[0]])
        a, b = ring @ u, ring @ v
        grid_u, grid_v = np.meshgrid(np.arange(a.min(), a.max(), ORCHARD_SPACING_M), np.arange(b.min(), b.max(), ORCHARD_SPACING_M))
        points = np.outer(grid_u.ravel(), u) + np.outer(grid_v.ravel(), v)
        inside = inside_ring(points, ring) & (np.hypot(*points.T) <= TREE_RADIUS_M)
        for x, y in points[inside]:
            rows.append([x, y, rng.uniform(3.5, 5.0), 2.0])
    return np.array(rows, np.float32).reshape(-1, 4)


def inside_ring(points: np.ndarray, ring: np.ndarray) -> np.ndarray:
    """Even-odd rule for each point against a closed ring."""
    x, y = points[:, 0][:, None], points[:, 1][:, None]
    ax, ay, bx, by = ring[:-1, 0], ring[:-1, 1], ring[1:, 0], ring[1:, 1]
    crosses = ((ay > y) != (by > y)) & (x < ax + (y - ay) * (bx - ax) / np.where(by == ay, 1e-9, by - ay))
    return crosses.sum(axis=1) % 2 == 1


def densify(xy: np.ndarray, step: float) -> np.ndarray:
    """The line with extra vertices so no segment is longer than `step`."""
    out = [xy[0]]
    for a, b in zip(xy[:-1], xy[1:]):
        n = max(1, int(np.ceil(np.hypot(*(b - a)) / step)))
        out += [a + (b - a) * (i / n) for i in range(1, n + 1)]
    return np.array(out)


def wall_pieces(walls: list[tuple[np.ndarray, int]]) -> np.ndarray:
    """float32 x0, y0, x1, y1, kind (0 wall, 1 hedge, 2 crest) for pieces within WALL_HALF_M."""
    rows = []
    for xy, kind in walls:
        xy = densify(xy, WALL_PIECE_M)
        for a, b in zip(xy[:-1], xy[1:]):
            if max(np.abs(a).max(), np.abs(b).max()) <= WALL_HALF_M:
                rows.append([*a, *b, kind])
    return np.array(rows, np.float32).reshape(-1, 5)


def leaf_grid(forests: list[tuple[np.ndarray, int]]) -> np.ndarray:
    """Level 0's leaf type from OSM forest outlines: 0 unknown, else LEAF."""
    from rasterio.features import rasterize
    from rasterio.transform import from_origin

    shapes = [({"type": "Polygon", "coordinates": [ring.tolist()]}, code) for ring, code in forests]
    if not shapes:
        return np.zeros((SIZE, SIZE), np.uint8)
    return rasterize(shapes, out_shape=(SIZE, SIZE), transform=from_origin(-HALF0_M, HALF0_M, 2 * HALF0_M / SIZE,
                                                                           2 * HALF0_M / SIZE), dtype=np.uint8)


def pack(array: np.ndarray) -> bytes:
    return gzip.compress(np.ascontiguousarray(array).tobytes(), compresslevel=6)


def encode(heights: np.ndarray) -> bytes:
    """uint16 = (h + 500 m) · 10, little-endian, gzip."""
    return pack(np.clip(np.round((heights + 500.0) * 10.0), 0, 65535).astype("<u2"))


def overpass(query: str) -> str:
    for attempt in range(4):
        request = urllib.request.Request(OVERPASS[attempt % len(OVERPASS)], headers={"User-Agent": USER_AGENT},
                                         data=urllib.parse.urlencode({"data": query}).encode())
        try:
            with urllib.request.urlopen(request, timeout=900) as response:
                return response.read().decode()
        except (urllib.error.HTTPError, TimeoutError) as error:
            # Overpass answers 429 and 500 to 504, or not at all, when it is busy.
            busy = isinstance(error, TimeoutError) or error.code in (429, 500, 502, 503, 504)
            if not busy or attempt == 3:
                raise
            time.sleep(60 * (attempt + 1))


def overpass_peaks(lat: float, lon: float, radius: float) -> list[tuple[float, float, str, str]]:
    text = overpass(f'[out:csv(::lat,::lon,ele,name;false)][timeout:600];'
                    f'node["natural"~"^(peak|volcano)$"]["name"](around:{radius:.0f},{lat},{lon});out;')
    rows = []
    for line in text.splitlines():
        parts = line.split("\t")
        if len(parts) == 4:
            rows.append((float(parts[0]), float(parts[1]), parts[2], parts[3]))
    return rows


def osm_ways(preset_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Highways and forests with a leaf type over the road levels, buildings over BUILDING_HALF_M,
    with their geometry. Cached per position, because a bake is rerun far more often than OSM
    changes."""
    cached = SOURCES / "osm" / f"{preset_id}_{lat:.5f}_{lon:.5f}.json"
    if not cached.exists():
        half = HALF0_M * 2 ** (ROAD_LEVELS - 1)
        w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
        bw, bs, be, bn = to_wgs84.transform_bounds(-BUILDING_HALF_M, -BUILDING_HALF_M, BUILDING_HALF_M,
                                                   BUILDING_HALF_M, densify_pts=21)
        box, near = f"{s},{w},{n},{e}", f"{bs},{bw},{bn},{be}"
        text = overpass(f'[out:json][timeout:600];(way["highway"]({box});'
                        f'way["landuse"="forest"]["leaf_type"]({box});way["natural"="wood"]["leaf_type"]({box});'
                        f'way["building"]({near}););out geom;')
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_text(text)
    return [way for way in json.loads(cached.read_text())["elements"] if way.get("geometry")]


def osm_water(preset_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Waterways and water areas over the water levels, cached like `osm_ways`. Lakes mapped as
    multipolygon relations are left to WorldCover, which sees anything that large."""
    cached = SOURCES / "osm" / f"{preset_id}_{lat:.5f}_{lon:.5f}_water.json"
    if not cached.exists():
        half = HALF0_M * 2 ** (WATER_LEVELS - 1)
        w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
        box = f"{s},{w},{n},{e}"
        text = overpass(f'[out:json][timeout:600];(way["waterway"~"^(river|stream|canal|riverbank)$"]({box});'
                        f'way["natural"="water"]({box});way["landuse"="reservoir"]({box}););out geom;')
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_text(text)
    return [way for way in json.loads(cached.read_text())["elements"] if way.get("geometry")]


def osm_features(preset_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Walls, hedges, orchards, aretes and ridges over the road levels, cached like `osm_ways`."""
    cached = SOURCES / "osm" / f"{preset_id}_{lat:.5f}_{lon:.5f}_features.json"
    if not cached.exists():
        half = HALF0_M * 2 ** (ROAD_LEVELS - 1)
        w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
        box = f"{s},{w},{n},{e}"
        text = overpass(f'[out:json][timeout:600];('
                        f'way["barrier"~"^(wall|retaining_wall|hedge)$"]({box});way["natural"~"^(arete|ridge)$"]({box});way["landuse"="orchard"]({box}););out geom;')
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_text(text)
    return [way for way in json.loads(cached.read_text())["elements"] if way.get("geometry")]


def landmark(tags: dict) -> str:
    """The building value, or the landmark it is: a lighthouse, or a ruined castle or tower."""
    if tags.get("man_made") == "lighthouse":
        return "lighthouse"
    if tags.get("ruins") == "yes" or tags.get("historic") in ("castle", "tower", "ruins") or tags["building"] == "ruins":
        return "ruin"
    return tags["building"]


def elevation_tag(text: str) -> float | None:
    match = re.match(r"\s*(-?\d+(?:[.,]\d+)?)", text.replace("'", ""))
    return float(match.group(1).replace(",", ".")) if match else None


def bake(preset: dict, skip_peaks: bool, dtm_levels: int) -> None:
    out = CACHE / preset["id"]
    out.mkdir(parents=True, exist_ok=True)
    lat, lon = preset["lat"], preset["lon"]
    frame = f"+proj=aeqd +lat_0={lat} +lon_0={lon} +datum=WGS84 +units=m +no_defs"
    to_wgs84 = Transformer.from_crs(CRS.from_proj4(frame), "EPSG:4326", always_xy=True)
    dtm = preset.get("dtm")
    sizes = []
    to_frame = Transformer.from_crs("EPSG:4326", CRS.from_proj4(frame), always_xy=True)

    def metres(way: dict) -> np.ndarray:
        return np.array(to_frame.transform([n["lon"] for n in way["geometry"]], [n["lat"] for n in way["geometry"]])).T

    ways = osm_ways(preset["id"], lat, lon, to_wgs84)
    codes = {name: i + 1 for i, name in enumerate(ROAD_WIDTHS)}
    roads = [(metres(w), codes[surface]) for w in ways
             if "highway" in w["tags"] and (surface := road_surface(w["tags"]))]
    forests = [(metres(w), LEAF[w["tags"]["leaf_type"]]) for w in ways
               if w["tags"].get("leaf_type") in LEAF and w["geometry"][0] == w["geometry"][-1]]
    print(f"  OSM: {len(roads)} roads, {len(forests)} forests with a leaf type, "
          f"{sum('building' in w['tags'] for w in ways)} buildings", flush=True)
    rivers, streams, lakes = [], [], []
    for way in osm_water(preset["id"], lat, lon, to_wgs84):
        tags, xy = way["tags"], metres(way)
        closed = way["geometry"][0] == way["geometry"][-1]
        if tags.get("waterway") in WATERWAY_WIDTHS and tags.get("tunnel") is None:
            width = elevation_tag(tags.get("width", "")) or WATERWAY_WIDTHS[tags["waterway"]]
            (streams if tags["waterway"] == "stream" else rivers).append((xy, width / 2))
        elif closed and tags.get("waterway") != "stream":
            lakes.append(xy)
    print(f"  OSM water: {len(rivers)} rivers and canals, {len(streams)} streams, {len(lakes)} areas", flush=True)
    features = osm_features(preset["id"], lat, lon, to_wgs84)
    tagged = lambda key, *values: [w for w in features if w["tags"].get(key) in values]
    walls = [(metres(w), 1 if w["tags"]["barrier"] == "hedge" else 0) for w in tagged("barrier", "wall", "retaining_wall", "hedge")]
    # Crests: the DEM rounds a knife-edge ridge off, so the page stands crags along it.
    walls += [(metres(w), 2) for w in tagged("natural", "arete", "ridge")]
    orchards = [metres(w) for w in tagged("landuse", "orchard") if w["geometry"][0] == w["geometry"][-1]]
    print(f"  OSM features: {len(walls)} walls and hedges, "
          f"{len(orchards)} orchards", flush=True)
    geology = GEOLOGY.get(dtm)
    units: dict[int, str] = {}
    habitats = nlcm(preset["id"], lat, lon, to_wgs84) if NLCM[0][0] <= lon <= NLCM[0][2] and NLCM[0][1] <= lat <= NLCM[0][3] else []
    if habitats:
        print(f"  land cover: {len(habitats)} polygons", flush=True)

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        dtm_paths = []
        if dtm:
            half = HALF0_M * 2 ** (dtm_levels - 1)
            dtm_paths = dtm_rasters(dtm, to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21))
        centre = slice(SIZE // 2 - 1, SIZE // 2 + 1)
        origin = {}
        for k in range(LEVELS):
            half = HALF0_M * 2**k
            bbox = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
            glo30 = glo30_level(k, frame, bbox, work)
            if k == 0:
                origin["glo30"] = float(glo30[centre, centre].mean())
            heights = glo30
            if dtm and k < dtm_levels:
                fine = warp(dtm_paths, frame, half, "average", overviews=False, work=work)[0]
                gaps = fine == NODATA
                fine[gaps] = glo30[gaps]
                if k == 0:
                    origin["dtm"] = float(fine[centre, centre].mean())
                heights = fine
                blob = encode(glo30)
                (out / f"glo30-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "glo30", len(blob)))
            classes = class_level(k, frame, bbox, heights, work)
            if heights is glo30:
                heights = sharpen_coast(heights, classes, half)
            blob = encode(heights)
            (out / f"h-{k}.bin.gz").write_bytes(blob)
            sizes.append((k, "h", len(blob)))
            blob = pack(classes)
            (out / f"class-{k}.bin.gz").write_bytes(blob)
            sizes.append((k, "class", len(blob)))
            if k < ROAD_LEVELS:
                road = road_field(roads, half)
                blob = pack(road)
                (out / f"road-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "road", len(blob)))
            if geology and k < ROCK_LEVELS:
                blob = pack(rock_level(dtm, k, frame, bbox, work, units))
                (out / f"rock-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "rock", len(blob)))
            if habitats and k < HABITAT_LEVELS:
                blob = pack(habitat_level(habitats, to_frame, half))
                (out / f"habitat-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "habitat", len(blob)))
            if k < WATER_LEVELS:
                water = water_field(rivers, streams, lakes, classes, half, work)
                blob = pack(water)
                (out / f"water-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "water", len(blob)))
            if dtm and k < dtm_levels:
                canopy = canopy_of(glo30, heights, classes, half)
                blob = pack(np.round(canopy * 4).astype(np.uint8))
                (out / f"canopy-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "canopy", len(blob)))
            if k == 0:
                heights0, classes0, road0, water0 = heights, classes, road, water
            if k == 1:
                heights1 = heights
                canopy0 = canopy if dtm else None
            print(f"  level {k}: ±{half / 1000:g} km, {2 * half / SIZE:.2f} m px, "
                  f"{heights.min():.0f}–{heights.max():.0f} m", flush=True)

        cdse_credentials()
        half = HALF0_M * 2 ** (S2_LEVELS - 1)
        quarters = s2_items(to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21), work)
        print(f"  Sentinel-2 mosaic tiles per quarter: {', '.join(f'Q{q} {len(v)}' for q, v in quarters.items())}")
        for k in range(S2_LEVELS):
            for q, texels in s2_level(k, frame, quarters, work).items():
                blob = pack(texels)
                (out / f"s2-q{q}-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, f"s2-q{q}", len(blob)))
            print(f"  colour level {k}", flush=True)

    trees = scatter_trees(classes0, road0, water0[..., 0], canopy0, leaf_grid(forests), preset.get("default_leaf", "mixed"))
    trees = np.concatenate([trees, orchard_trees(orchards)])
    blob = pack(trees)
    (out / "trees.bin.gz").write_bytes(blob)
    sizes.append(("-", "trees", len(blob)))
    buildings = []
    for way in ways:
        ring = metres(way)
        tags = way["tags"]
        if "building" not in tags or len(ring) < 4 or np.abs(ring).max() > BUILDING_HALF_M:
            continue
        # Level 0 holds ±1.25 km; past it a footprint takes its ground from level 1, not the edge.
        inner = np.abs(ring).max() < HALF0_M
        base = float(sample(heights0 if inner else heights1, HALF0_M * (1 if inner else 2), ring[:, 0], ring[:, 1]).min())
        buildings.append([round(base, 1), landmark(tags), elevation_tag(tags.get("building:levels", "")),
                          elevation_tag(tags.get("height", "")), [round(float(v), 1) for v in ring[:-1].ravel()]])
    blob = json.dumps(buildings, separators=(",", ":")).encode()
    (out / "buildings.json").write_bytes(blob)
    sizes.append(("-", "buildings", len(blob)))

    blob = pack(wall_pieces(walls))
    (out / "walls.bin.gz").write_bytes(blob)
    sizes.append(("-", "walls", len(blob)))
    print(f"  {len(trees)} trees, {len(buildings)} buildings", flush=True)

    ground = preset.get("ground_m", origin.get("dtm", origin["glo30"]))
    print(f"  ground {ground:.1f} m; at the origin the DTM says {origin.get('dtm', float('nan')):.1f} m, "
          f"GLO-30 {origin['glo30']:.1f} m")

    credits = [CREDITS["glo30"]] + ([CREDITS[dtm]] if dtm else []) + [CREDITS["worldcover"], CREDITS["s2"]]
    credits += [geology[2]] if geology else []
    credits += [NLCM[2]] if habitats else []
    if not skip_peaks:
        radius = HALF0_M * 2 ** (LEVELS - 1)
        raw = overpass_peaks(lat, lon, radius)
        to_frame = Transformer.from_crs("EPSG:4326", CRS.from_proj4(frame), always_xy=True)
        xs, ys = to_frame.transform([r[1] for r in raw], [r[0] for r in raw])
        peaks = [[round(x), round(y), elevation_tag(ele), name]
                 for x, y, (_, _, ele, name) in zip(xs, ys, raw)]
        blob = json.dumps(peaks, ensure_ascii=False, separators=(",", ":")).encode()
        (out / "peaks.json").write_bytes(blob)
        sizes.append(("-", "peaks", len(blob)))
    if (out / "peaks.json").exists():
        credits.append(CREDITS["osm"])

    meta = {key: preset[key] for key in ("id", "lat", "lon", "heading", "pitch") if key in preset}
    above = preset.get("eye_above_ground_m", PHONE_M)
    meta.update(ground_m=round(float(ground), 2), eye_m=round(float(ground) + above, 2), eye_above_ground_m=above,
                levels=LEVELS, size=SIZE, half0_m=HALF0_M, dtm_levels=dtm_levels if dtm else 0,
                dtm=dtm, s2_levels=S2_LEVELS, road_levels=ROAD_LEVELS, water_levels=WATER_LEVELS, shore_reach_m=SHORE_REACH_M,
                rock_levels=ROCK_LEVELS if geology else 0, habitat_levels=HABITAT_LEVELS if habitats else 0,
                credits=credits)
    if "photo_source" in preset:
        shutil.copyfile(preset["photo_source"], out / "photo.jpg")
        meta["photo"] = preset["photo"]
        credits.append(preset["photo"]["credit"])
    (out / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1))

    print(f"\n{'level':>5} {'layer':>6} {'bytes':>10}")
    for row in sizes:
        print(f"{row[0]!s:>5} {row[1]:>6} {row[2]:>10,}")
    print(f"{'':>5} {'total':>6} {sum(row[2] for row in sizes):>10,}")
    season = sum(row[2] for row in sizes
                 if row[1] in ("h", "class", "road", "water", "canopy", "trees", "buildings", "peaks", "s2-q3", "rock", "habitat", "walls"))
    print(f"one season without the GLO-30 comparison: {season:,} bytes")
    print(f"\nwrote {out}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("preset")
    parser.add_argument("--skip-peaks", action="store_true", help="keep the peaks.json of an earlier bake")
    parser.add_argument("--dtm-levels", type=int, default=DTM_LEVELS,
                        help=f"levels that take the national DTM (default {DTM_LEVELS}: ±10 km)")
    args = parser.parse_args()
    bake(load_preset(args.preset), args.skip_peaks, args.dtm_levels)


if __name__ == "__main__":
    main()
