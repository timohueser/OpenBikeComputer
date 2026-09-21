import os
import pathlib
"""Fit a pinhole camera to a photo's skyline, then overlay our DEM profile."""
import math, subprocess, sys
import numpy as np
from PIL import Image, ImageOps, ImageDraw
from scipy.ndimage import binary_closing, label, uniform_filter

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")

SP = WORK
BIN = ROOT / "pano/target/release"
OBCD = f"{WORK}/eng9.obcd"
TOOL = f"{BIN}/obctsky"

def load(path, width=960):
    im = ImageOps.exif_transpose(Image.open(path)).convert("RGB")
    im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
    return im

def sky_mask(im, blue_ratio=1.15, floor=0.95):
    """Sky is markedly blue and no darker than `floor` times the zenith.

    The gate is relative because one absolute threshold cannot serve two exposures: a high sky
    photographs deep and dark (blue 145 at 2600 m) while hazy rock in a brighter frame reaches
    blue 197. The top rows of these frames are always sky, so they set the scale.

    `floor` still has to be set per photo. Hazy distant terrain and the sky just above it differ
    by about 50 counts of blue in one of these four frames and by 150 in another, so no single
    value separates them everywhere. Each case in the harness records its own.
    """
    a = np.asarray(im).astype(np.float32)
    r, b = np.maximum(a[..., 0], 1.0), a[..., 2]
    zenith = float(np.median(b[: max(4, a.shape[0] // 100)]))
    return (b > floor * zenith) & (b / r > blue_ratio)


def photo_skyline(im, run=15, **kw):
    """First non-sky row per column, scanning down; -1 where the column never leaves the sky."""
    m = sky_mask(im, **kw)
    h, w = m.shape
    out = np.full(w, -1)
    for x in range(w):
        col = m[:, x]
        # ignore isolated dark specks (birds, contrails) — need `run` consecutive non-sky rows
        for i in np.nonzero(~col)[0]:
            if i + run <= h and (~col[i:i + run]).all():
                out[x] = i
                break
    return out


def dem_skyline(lat, lon, eye_off, b0, b1, step, far=42000.0):
    r = subprocess.run([TOOL, OBCD, f"{lat:.6f}", f"{lon:.6f}", str(eye_off),
                        f"{b0:.4f}", f"{b1:.4f}", f"{step}", str(far)],
                       capture_output=True, text=True, check=True)
    bs, els = [], []
    for line in r.stdout.splitlines():
        p = line.split()
        bs.append(float(p[0])); els.append(float(p[1]))
    return np.array(bs), np.array(els)

def project(bearings, elevs, yaw, pitch, roll, f, cx, cy):
    """Pinhole projection of world directions given as (bearing, elevation) in degrees."""
    b = np.radians(bearings); e = np.radians(elevs)
    d = np.stack([np.sin(b)*np.cos(e), np.cos(b)*np.cos(e), np.sin(e)], axis=-1)  # east, north, up
    Y, P, R = math.radians(yaw), math.radians(pitch), math.radians(roll)
    fwd = np.array([math.sin(Y)*math.cos(P), math.cos(Y)*math.cos(P), math.sin(P)])
    r0 = np.array([math.cos(Y), -math.sin(Y), 0.0])
    u0 = np.cross(r0, fwd)
    right = r0*math.cos(R) + u0*math.sin(R)
    up = -r0*math.sin(R) + u0*math.cos(R)
    z = d @ fwd
    with np.errstate(divide='ignore', invalid='ignore'):
        u = cx + f * (d @ right) / z
        v = cy - f * (d @ up) / z
    u[z <= 0.05] = np.nan; v[z <= 0.05] = np.nan
    return u, v

def residual(params, bearings, elevs, xs, ys, cx, cy):
    yaw, pitch, roll, f = params
    u, v = project(bearings, elevs, yaw, pitch, roll, f, cx, cy)
    ok = np.isfinite(u) & np.isfinite(v)
    if ok.sum() < 20: return 1e9, None
    order = np.argsort(u[ok])
    uu, vv = u[ok][order], v[ok][order]
    pred = np.interp(xs, uu, vv, left=np.nan, right=np.nan)
    m = np.isfinite(pred)
    if m.sum() < len(xs) * 0.6: return 1e9, None
    err = pred[m] - ys[m]
    return float(np.sqrt(np.mean(err**2))), err
