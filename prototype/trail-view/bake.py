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
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

import numpy as np
import rasterio
from pyproj import CRS, Transformer

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
CACHE = Path.home() / ".cache/obc/trail-view"
SOURCES = CACHE / "_sources"

LEVELS = 9
SIZE = 1024
HALF0_M = 1250.0
# The national DTM covers k ≤ 2, ±5 km, unless --dtm-levels says otherwise.
DTM_LEVELS = 3
# The height above the ground the Peak View photographs were taken from.
PHONE_M = 1.6
NODATA = -32768.0

GLO30_LOCAL = Path.home() / ".cache/obcm/dem"
GLO30_S3 = "https://copernicus-dem-30m.s3.amazonaws.com"
GLO30_PX_M = 30.0
PHOTO_JSON = REPO / "fixtures/sources/peak-view/photos"
OVERPASS = "https://overpass-api.de/api/interpreter"
USER_AGENT = "OpenBikeComputer trail view prototype (https://github.com/timohueser/OpenBikeComputer)"

GDAL_ENV = {**os.environ, "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
            "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif", "GDAL_HTTP_MAX_RETRY": "4",
            "GDAL_HTTP_RETRY_DELAY": "5"}

CREDITS = {
    "glo30": "Copernicus DEM GLO-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the European Union and ESA",
    "ch": "swissALTI3D © swisstopo",
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


def warp(inputs, frame: str, half: float, resampling: str, overviews: bool, work: Path) -> np.ndarray:
    out = work / "warp.tif"
    cmd = ["gdalwarp", "-q", "-overwrite", "-multi", "-wo", "NUM_THREADS=ALL_CPUS",
           "-t_srs", frame, "-te", str(-half), str(-half), str(half), str(half),
           "-ts", str(SIZE), str(SIZE), "-r", resampling, "-ot", "Float32", "-dstnodata", str(NODATA)]
    if not overviews:
        cmd += ["-ovr", "NONE"]
    subprocess.run(cmd + [str(p) for p in inputs] + [str(out)], check=True, env=GDAL_ENV)
    with rasterio.open(out) as src:
        return src.read(1)


def glo30_level(k: int, frame: str, bbox, work: Path) -> np.ndarray:
    half = HALF0_M * 2**k
    px = 2 * half / SIZE
    # Upsampling 30 m cells needs interpolation; downsampling keeps each summit with max.
    resampling = "bilinear" if px < GLO30_PX_M else "max"
    # COG overviews are averaged, which flattens summits; past k = 5 (156 m px) that costs
    # under 0.03° and saves reading full-resolution remote tiles.
    heights = warp(glo30_tiles(bbox), frame, half, resampling, overviews=k > 5, work=work)
    # Missing tiles are sea.
    heights[heights == NODATA] = 0.0
    return heights


def dtm_fetch(key: str, bbox) -> list[Path]:
    sys.path.insert(0, str(REPO / "host/obc-dem/reference"))
    try:
        from ingest.sources import SOURCES as DTM_SOURCES
    finally:
        sys.path.pop(0)
    return DTM_SOURCES[key].fetch(bbox, SOURCES / key)


def encode(heights: np.ndarray) -> bytes:
    """uint16 = (h + 500 m) · 10, little-endian, gzip."""
    raw = np.clip(np.round((heights + 500.0) * 10.0), 0, 65535).astype("<u2").tobytes()
    return gzip.compress(raw, compresslevel=6)


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
            dtm_paths = dtm_fetch(dtm, to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21))
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
                fine = warp(dtm_paths, frame, half, "average", overviews=False, work=work)
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
            print(f"  level {k}: ±{half / 1000:g} km, {2 * half / SIZE:.2f} m px, "
                  f"{heights.min():.0f}–{heights.max():.0f} m", flush=True)

    ground = preset.get("ground_m", origin.get("dtm", origin["glo30"]))
    print(f"  ground {ground:.1f} m; at the origin the DTM says {origin.get('dtm', float('nan')):.1f} m, "
          f"GLO-30 {origin['glo30']:.1f} m")

    credits = [CREDITS["glo30"]] + ([CREDITS[dtm]] if dtm else [])
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
                dtm=dtm, credits=credits)
    if "photo_source" in preset:
        shutil.copyfile(preset["photo_source"], out / "photo.jpg")
        meta["photo"] = preset["photo"]
        credits.append(CREDITS["photo"])
    (out / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1))

    print(f"\n{'level':>5} {'layer':>6} {'bytes':>10}")
    for row in sizes:
        print(f"{row[0]!s:>5} {row[1]:>6} {row[2]:>10,}")
    print(f"{'':>5} {'total':>6} {sum(row[2] for row in sizes):>10,}")
    print(f"\nwrote {out}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("preset")
    parser.add_argument("--skip-peaks", action="store_true", help="keep the peaks.json of an earlier bake")
    parser.add_argument("--dtm-levels", type=int, default=DTM_LEVELS,
                        help=f"levels that take the national DTM (default {DTM_LEVELS}: ±5 km)")
    args = parser.parse_args()
    bake(load_preset(args.preset), args.skip_peaks, args.dtm_levels)


if __name__ == "__main__":
    main()
