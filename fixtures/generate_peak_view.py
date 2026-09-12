"""Bake geographic OBCT terrain fixtures for runtime Peak View rendering.

Requires numpy and Pillow. Reads cached Terrarium tiles unless --download is set.
"""
import argparse
import json
import math
import struct
import subprocess
import tempfile
from pathlib import Path
from urllib.request import urlopen

import numpy as np
from PIL import Image

CACHE = Path.home() / ".cache/openbikecomputer/terrarium"
DOWNLOAD = False
ROOT = Path(__file__).resolve().parents[1]


def fetch_tile(z, x, y):
    path = CACHE / str(z) / str(x) / f"{y}.png"
    if not path.exists():
        if not DOWNLOAD:
            raise FileNotFoundError(f"{path}: use --download to fetch missing terrain")
        url = f"https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
        with urlopen(url, timeout=30) as response:
            data = response.read()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    with Image.open(path) as image:
        rgb = np.asarray(image.convert("RGB"), dtype=np.float64)
    return rgb[..., 0] * 256 + rgb[..., 1] + rgb[..., 2] / 256 - 32768


def tile_xy(lat, lon, z):
    n = 2 ** z
    x = (lon + 180) / 360 * n
    y = (1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n
    return x, y


class Mosaic:
    """A square Web-Mercator mosaic around the observer at one zoom, bilinear sampling."""

    def __init__(self, lat, lon, z, radius_m):
        self.z = z
        cx, cy = tile_xy(lat, lon, z)
        m_per_tile = 40075016.686 * math.cos(math.radians(lat)) / (2 ** z)
        pad = int(math.ceil(radius_m / m_per_tile)) + 1
        self.x0, self.y0 = int(cx) - pad, int(cy) - pad
        n = 2 * pad + 1
        rows = []
        for ty in range(self.y0, self.y0 + n):
            rows.append(np.concatenate([fetch_tile(z, tx, ty) for tx in range(self.x0, self.x0 + n)], axis=1))
        self.dem = np.concatenate(rows, axis=0)

    def sample(self, lat, lon):
        n = 2 ** self.z
        x = (lon + 180) / 360 * n
        latr = np.radians(lat)
        y = (1 - np.log(np.tan(latr) + 1 / np.cos(latr)) / np.pi) / 2 * n
        px = (x - self.x0) * 256 - 0.5
        py = (y - self.y0) * 256 - 0.5
        h, w = self.dem.shape
        px = np.clip(px, 0, w - 1.001)
        py = np.clip(py, 0, h - 1.001)
        ix, iy = px.astype(int), py.astype(int)
        fx, fy = px - ix, py - iy
        d = self.dem
        return (d[iy, ix] * (1 - fx) * (1 - fy) + d[iy, ix + 1] * fx * (1 - fy)
                + d[iy + 1, ix] * (1 - fx) * fy + d[iy + 1, ix + 1] * fx * fy)


# Geographic fixture levels. No observer-dependent shading or visibility is stored.
LEVELS = [(9, 12, 28000), (10, 11, 53000), (11, 11, 103000)]

def bake_level(site, posting_log2, zoom, radius, output):
    lat, lon = site["observer_lat"] / 1e6, site["observer_lon"] / 1e6
    mosaic = Mosaic(lat, lon, zoom, radius + 2000)
    origin, cell_log2 = -(1 << 28), 16
    cell = 1 << cell_log2
    dy = radius / 111320 * 1e6
    dx = dy / math.cos(math.radians(lat))
    imin = math.floor((lat * 1e6 - dy - origin) / cell)
    imax = math.floor((lat * 1e6 + dy - origin) / cell)
    jmin = math.floor((lon * 1e6 - dx - origin) / cell)
    jmax = math.floor((lon * 1e6 + dx - origin) / cell)
    rows, cols = imax - imin + 1, jmax - jmin + 1
    n = 1 << (cell_log2 - posting_log2)
    offset = (32 + rows * cols * 4 + 511) // 512 * 512
    header = struct.pack("<4sBBBBIIHHI8x", b"OBCT", 1, posting_log2, cell_log2, 0, imin, jmin, rows, cols, 32)
    with output.open("wb") as f:
        f.write(header)
        for i in range(rows * cols): f.write(struct.pack("<I", offset + i * n * n * 2))
        f.write(bytes(offset - f.tell()))
        for ci in range(imin, imax + 1):
            for cj in range(jmin, jmax + 1):
                ys = (origin + ci * cell + np.arange(n) * (1 << posting_log2)) / 1e6
                xs = (origin + cj * cell + np.arange(n) * (1 << posting_log2)) / 1e6
                heights = np.rint(mosaic.sample(ys[:, None], xs[None, :])).astype("<i2")
                # Tiles and samples both advance north first in the row dimension.
                tiled = heights.reshape(n // 16, 16, n // 16, 16).transpose(0, 2, 1, 3)
                f.write(tiled.tobytes())
    print(f"{output.name}: {output.stat().st_size} bytes", flush=True)

def read_level(path):
    data = path.read_bytes()
    magic, version, posting, cell, flags, iy, ix, rows, cols, directory = struct.unpack_from("<4sBBBBIIHHI", data)
    if (magic, version, flags) != (b"OBCT", 1, 0):
        raise ValueError(f"{path}: expected geographic OBCT v1 source heights")
    n = 1 << (cell - posting)
    grid = np.empty((rows * n, cols * n), dtype=np.int16)
    for y in range(rows):
        for x in range(cols):
            offset = struct.unpack_from("<I", data, directory + 4 * (y * cols + x))[0]
            if offset == 0xffffffff:
                raise ValueError(f"{path}: source cell is missing")
            tile = np.frombuffer(data, dtype="<i2", count=n * n, offset=offset)
            grid[y*n:(y+1)*n, x*n:(x+1)*n] = tile.reshape(n//16, n//16, 16, 16).transpose(0, 2, 1, 3).reshape(n, n)
    if np.any(grid == -32768):
        raise ValueError(f"{path}: source height is missing")
    return dict(grid=grid, posting=posting, cell=cell, iy=iy, ix=ix, rows=rows, cols=cols,
                lat=-(1 << 28) + (iy << cell), lon=-(1 << 28) + (ix << cell))


def combine_levels(source, output, site):
    """Put the finest available geographic heights on one native lattice.

    Coarse outer sources extend coverage; interpolation does not add terrain detail.
    Bounds and lower-resolution levels are then made by the production OBCT baker.
    """
    levels = [read_level(source / f"{site}-{posting}.obcd") for posting, _, _ in LEVELS]
    coarse = levels[-1]
    n = 1 << (coarse["cell"] - 9)
    rows, cols = coarse["rows"], coarse["cols"]
    offset = (32 + rows * cols * 4 + 511) // 512 * 512
    header = struct.pack("<4sBBBBIIHHI8x", b"OBCT", 1, 9, coarse["cell"], 0,
                         coarse["iy"], coarse["ix"], rows, cols, 32)
    with output.open("wb") as f:
        f.write(header)
        for i in range(rows * cols):
            f.write(struct.pack("<I", offset + i * n * n * 2))
        f.write(bytes(offset - f.tell()))
        for cy in range(rows):
            for cx in range(cols):
                lat = coarse["lat"] + (cy * n + np.arange(n))[:, None] * 512
                lon = coarse["lon"] + (cx * n + np.arange(n))[None, :] * 512
                heights = np.zeros((n, n), dtype=np.float64)
                for level in reversed(levels):
                    g = level["grid"]
                    py = (lat - level["lat"]) / (1 << level["posting"])
                    px = (lon - level["lon"]) / (1 << level["posting"])
                    valid = (py >= 0) & (py <= g.shape[0] - 1) & (px >= 0) & (px <= g.shape[1] - 1)
                    if level is coarse:
                        valid = np.ones((n, n), dtype=bool)
                    py = np.clip(py, 0, g.shape[0] - 1)
                    px = np.clip(px, 0, g.shape[1] - 1)
                    y, x = py.astype(np.intp), px.astype(np.intp)
                    fy, fx = py - y, px - x
                    y1, x1 = np.minimum(y + 1, g.shape[0] - 1), np.minimum(x + 1, g.shape[1] - 1)
                    h = (g[y, x] * (1-fy) * (1-fx) + g[y1, x] * fy * (1-fx)
                         + g[y, x1] * (1-fy) * fx + g[y1, x1] * fy * fx)
                    heights[valid] = h[valid]
                heights = np.rint(heights).astype("<i2")
                f.write(heights.reshape(n//16, 16, n//16, 16).transpose(0, 2, 1, 3).tobytes())


def catalog_source(sites):
    output = ["// Generated observer metadata. Geographic terrain lives in the fixture cache.",
              "use obc_app::{PeakName, PeakViewPeak, PeakViewProfile};"]
    for site in sites:
        output.append(f"pub(super) static {site['key'].upper()}: PeakViewProfile<'static> = PeakViewProfile {{")
        output.extend(f"    {key}: {json.dumps(value, ensure_ascii=False)}," for key, value in site.items() if key not in ("key", "peaks"))
        output.append("    peaks: &[")
        for peak in sorted(site["peaks"], key=lambda p: p["azimuth_q4"]):
            # These legacy fixture inputs retained projected bearings, not original OSM coordinates.
            angle = math.radians(peak["azimuth_q4"] / 4)
            lat = peak.get("lat", round(site["observer_lat"] + peak["distance_m"] * math.cos(angle) / 0.11132))
            lon = peak.get("lon", round(site["observer_lon"] + peak["distance_m"] * math.sin(angle) /
                           (0.11132 * math.cos(math.radians(site["observer_lat"] / 1e6)))))
            values = dict(peak, lat=lat, lon=lon, visible=peak.get("visible", True), angle_q4=peak.get("angle_q4", 0))
            fields = []
            for key, value in values.items():
                literal = json.dumps(value, ensure_ascii=False)
                if key == "name": literal = f"PeakName::new({literal})"
                if key == "elevation_m": literal = f"Some({literal})" if value is not None else "None"
                fields.append(f"{key}: {literal}")
            output.append("        PeakViewPeak { " + ", ".join(fields) + " },")
        output.append("    ],\n};")
    return "\n".join(output) + "\n"


def main():
    global CACHE, DOWNLOAD
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=CACHE)
    parser.add_argument("--download", action="store_true")
    parser.add_argument("--terrain-dir", type=Path, default=Path.home() / ".cache/openbikecomputer/peak-view")
    parser.add_argument("--source-levels", type=Path, help="reuse geographic 9/10/11 source files from an earlier bake")
    parser.add_argument("--dem-tool", type=Path, default=ROOT / "target/release/obc-dem")
    args = parser.parse_args()
    if not args.dem_tool.is_file():
        parser.error(f"{args.dem_tool}: run cargo build --release -p obc-dem first")
    CACHE, DOWNLOAD = args.cache, args.download
    args.terrain_dir.mkdir(parents=True, exist_ok=True)
    sites = json.loads((ROOT / "fixtures/sources/peak-view/locations.json").read_text())
    for site in sites:
        with tempfile.TemporaryDirectory(prefix=".peak-view-", dir=args.terrain_dir) as temporary:
            work = Path(temporary)
            source = args.source_levels or work
            if args.source_levels is None:
                for posting, zoom, radius in LEVELS:
                    bake_level(site, posting, zoom, radius, source / f"{site['key']}-{posting}.obcd")
            native = work / "native.obcd"
            combined = work / f"{site['key']}.obcd"
            combine_levels(source, native, site["key"])
            subprocess.run([str(args.dem_tool.resolve()), "surface", str(native), str(combined)], check=True)
            combined.replace(args.terrain_dir / combined.name)
            print(f"{combined.name}: {(args.terrain_dir / combined.name).stat().st_size} bytes", flush=True)
    (ROOT / "fixtures/sources/peak-view/catalog.rs").write_text(catalog_source(sites))

if __name__ == "__main__": main()
