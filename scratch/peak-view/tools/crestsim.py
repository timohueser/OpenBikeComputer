import os
import pathlib
"""Simulate a crest-correction layer: our 2^9 lattice, corrected at nodes the LiDAR says we undershoot."""
import math, sys
import numpy as np
import rasterio

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")

SP = WORK
POST = 512               # 2^9 microdegrees, the v1 posting
ORIGIN = -(1 << 28)      # OBCT grid origin, microdegrees
CURV = 0.87 / (2 * 6_371_000.0)

def node_coords(lo_udeg, hi_udeg):
    i0 = math.floor((lo_udeg - ORIGIN) / POST)
    i1 = math.ceil((hi_udeg - ORIGIN) / POST)
    idx = np.arange(i0, i1 + 1)
    return idx, (ORIGIN + idx * POST) / 1e6

class Lattice:
    """Heights on the OBCT lattice, bilinearly sampled — the surface the renderer integrates."""
    def __init__(self, lat_idx, lat, lon_idx, lon, grid):
        self.lat_idx, self.lat, self.lon_idx, self.lon, self.g = lat_idx, lat, lon_idx, lon, grid
    def sample(self, la, lo):
        fy = (la * 1e6 - ORIGIN) / POST - self.lat_idx[0]
        fx = (lo * 1e6 - ORIGIN) / POST - self.lon_idx[0]
        iy = np.clip(fy.astype(int), 0, self.g.shape[0] - 2)
        ix = np.clip(fx.astype(int), 0, self.g.shape[1] - 2)
        ty, tx = fy - iy, fx - ix
        g = self.g
        return (g[iy, ix]*(1-tx)*(1-ty) + g[iy, ix+1]*tx*(1-ty)
                + g[iy+1, ix]*(1-tx)*ty + g[iy+1, ix+1]*tx*ty)

def build(src_path, lat_range, lon_range):
    ds = rasterio.open(src_path)
    band = ds.read(1).astype(np.float64)
    if ds.nodata is not None:
        band[band == ds.nodata] = np.nan
    lat_idx, lat = node_coords(*lat_range)
    lon_idx, lon = node_coords(*lon_range)
    LO, LA = np.meshgrid(lon, lat)
    rows, cols = ~ds.transform * (LO, LA)
    r = np.clip(rows, 0, ds.width - 1.001); c = np.clip(cols, 0, ds.height - 1.001)
    i0, j0 = c.astype(int), r.astype(int)
    fy, fx = c - i0, r - j0
    g = (band[i0, j0]*(1-fx)*(1-fy) + band[i0, j0+1]*fx*(1-fy)
         + band[i0+1, j0]*(1-fx)*fy + band[i0+1, j0+1]*fx*fy)
    ds.close()
    return Lattice(lat_idx, lat, lon_idx, lon, g)

def skyline(sampler, lat, lon, eye, bearings, far=12000.0, step=4.0):
    d = np.arange(20.0, far, step)
    out_a, out_d = [], []
    coslat = math.cos(math.radians(lat))
    for b in bearings:
        s, c = math.sin(math.radians(b)), math.cos(math.radians(b))
        h = sampler(lat + c * d / 111320.0, lon + s * d / (111320.0 * coslat))
        slope = (h - eye) / d - CURV * d
        k = int(np.nanargmax(slope))
        out_a.append(math.degrees(math.atan(slope[k]))); out_d.append(d[k])
    return np.array(out_a), np.array(out_d)
