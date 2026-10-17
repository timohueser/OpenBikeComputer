# /// script
# requires-python = ">=3.12"
# dependencies = ["rasterio==1.5.2", "pyproj==3.8.0", "numpy==2.5.3"]
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
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

import numpy as np
import rasterio
from pyproj import CRS, Transformer

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path[:0] = [str(REPO / "host/obc-dem/reference"), str(REPO)]
from ingest.sources import SOURCES as DTM_SOURCES  # noqa: E402
from ingest.sources.stac import stac_items  # noqa: E402
from tools.planner_snow import cdse_credentials  # noqa: E402

CACHE = Path.home() / ".cache/obc/trail-view"
SOURCES = CACHE / "_sources"

LEVELS = 9
SIZE = 1024
HALF0_M = 1250.0
# The national DTM covers k ≤ 3, ±10 km. The owner chose it after ±5 km put the Rigidalstock
# ridge, 4.7 km from Brunni, into the blend toward GLO-30.
DTM_LEVELS = 4
# The height above the ground the Peak View photographs were taken from.
PHONE_M = 1.6
NODATA = -32768.0

GLO30_LOCAL = Path.home() / ".cache/obcm/dem"
GLO30_S3 = "https://copernicus-dem-30m.s3.amazonaws.com"
GLO30_PX_M = 30.0
PHOTO_JSON = REPO / "fixtures/sources/peak-view/photos"
OVERPASS = "https://overpass-api.de/api/interpreter"
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

GDAL_OPTIONS = {"GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR", "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif,.vrt",
                "GDAL_HTTP_MAX_RETRY": "4", "GDAL_HTTP_RETRY_DELAY": "5"}

CREDITS = {
    "glo30": "Copernicus DEM GLO-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the European Union and ESA",
    "ch": "swissALTI3D © swisstopo",
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
    if name := preset.get("photo"):
        photo = json.loads((PHOTO_JSON / f"{name}.json").read_text())
        preset.update(lat=photo["lat"], lon=photo["lon"], ground_m=photo["ground_m"],
                      heading=photo["heading_deg"], pitch=photo["pitch_deg"])
        preset["photo"] = {key: photo[key] for key in
                           ("heading_deg", "pitch_deg", "lens_mm", "columns", "rms_limit_deg", "subject")}
        preset["photo"]["file"] = "photo.jpg"
        preset["photo_source"] = fixture_photo(name)
    return preset


def glo30_tiles(bbox) -> list[str]:
    """Every GLO-30 tile the box touches: the local copy if there is one, else the public COG."""
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
            if local.exists():
                tiles.append(str(local))
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
    heights = warp(glo30_tiles(bbox), frame, half, resampling, overviews=k > 5, work=work)[0]
    # Missing tiles are sea.
    heights[heights == NODATA] = 0.0
    return heights


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
    return np.stack([index, np.where(index == WATER, 255, 0).astype(np.uint8)], axis=-1)


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


def pack(array: np.ndarray) -> bytes:
    return gzip.compress(np.ascontiguousarray(array).tobytes(), compresslevel=6)


def encode(heights: np.ndarray) -> bytes:
    """uint16 = (h + 500 m) · 10, little-endian, gzip."""
    return pack(np.clip(np.round((heights + 500.0) * 10.0), 0, 65535).astype("<u2"))


def overpass_peaks(lat: float, lon: float, radius: float) -> list[tuple[float, float, str, str]]:
    query = (f'[out:csv(::lat,::lon,ele,name;false)][timeout:600];'
             f'node["natural"="peak"]["name"](around:{radius:.0f},{lat},{lon});out;')
    request = urllib.request.Request(OVERPASS, data=urllib.parse.urlencode({"data": query}).encode(),
                                     headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=900) as response:
        text = response.read().decode()
    rows = []
    for line in text.splitlines():
        parts = line.split("\t")
        if len(parts) == 4:
            rows.append((float(parts[0]), float(parts[1]), parts[2], parts[3]))
    return rows


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

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        dtm_paths = []
        if dtm:
            half = HALF0_M * 2 ** (dtm_levels - 1)
            dtm_paths = DTM_SOURCES[dtm].fetch(to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21),
                                               SOURCES / dtm)
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
            blob = encode(heights)
            (out / f"h-{k}.bin.gz").write_bytes(blob)
            sizes.append((k, "h", len(blob)))
            blob = pack(class_level(k, frame, bbox, heights, work))
            (out / f"class-{k}.bin.gz").write_bytes(blob)
            sizes.append((k, "class", len(blob)))
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

    ground = preset.get("ground_m", origin.get("dtm", origin["glo30"]))
    print(f"  ground {ground:.1f} m; at the origin the DTM says {origin.get('dtm', float('nan')):.1f} m, "
          f"GLO-30 {origin['glo30']:.1f} m")

    credits = [CREDITS["glo30"]] + ([CREDITS[dtm]] if dtm else []) + [CREDITS["worldcover"], CREDITS["s2"]]
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
    meta.update(ground_m=round(float(ground), 2), eye_m=round(float(ground) + PHONE_M, 2),
                levels=LEVELS, size=SIZE, half0_m=HALF0_M, dtm_levels=dtm_levels if dtm else 0,
                dtm=dtm, s2_levels=S2_LEVELS, credits=credits)
    if "photo_source" in preset:
        shutil.copyfile(preset["photo_source"], out / "photo.jpg")
        meta["photo"] = preset["photo"]
        credits.append(CREDITS["photo"])
    (out / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1))

    print(f"\n{'level':>5} {'layer':>6} {'bytes':>10}")
    for row in sizes:
        print(f"{row[0]!s:>5} {row[1]:>6} {row[2]:>10,}")
    print(f"{'':>5} {'total':>6} {sum(row[2] for row in sizes):>10,}")
    season = sum(row[2] for row in sizes if row[1] in ("h", "class", "peaks", "s2-q3"))
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
