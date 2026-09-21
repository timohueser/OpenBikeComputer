import os
import pathlib
"""Fit a pinhole camera to a photo's skyline and draw our DEM profile over it."""
import math, sys, subprocess
import numpy as np
from PIL import ImageDraw
from scipy.optimize import minimize
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from fit import load, photo_skyline, dem_skyline, project, SP

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")

def huber(e, k=6.0):
    a = np.abs(e)
    return np.where(a <= k, 0.5 * e * e, k * (a - 0.5 * k))

class Fitter:
    def __init__(self, bearings, elevs, xs, ys, cx, cy):
        self.b, self.e, self.xs, self.ys, self.cx, self.cy = bearings, elevs, xs, ys, cx, cy
    def predict(self, p):
        u, v = project(self.b, self.e, p[0], p[1], p[2], p[3], self.cx, self.cy)
        ok = np.isfinite(u) & np.isfinite(v)
        if ok.sum() < 20: return None
        o = np.argsort(u[ok])
        uu, vv = u[ok][o], v[ok][o]
        keep = np.concatenate([[True], np.diff(uu) > 0])
        return np.interp(self.xs, uu[keep], vv[keep], left=np.nan, right=np.nan)
    bounds = None
    def cost(self, p):
        if self.bounds is not None:
            for v, (lo, hi) in zip(p, self.bounds):
                if not (lo <= v <= hi): return 1e12
        pred = self.predict(p)
        if pred is None: return 1e12
        m = np.isfinite(pred)
        # Every detected column must be covered, or a partial match can win on coverage alone.
        if m.sum() < len(self.xs) * 0.97: return 1e12
        return float(huber(pred[m] - self.ys[m]).sum() / m.sum())
    def rms(self, p):
        pred = self.predict(p)
        m = np.isfinite(pred)
        e = pred[m] - self.ys[m]
        # robust: drop the worst 10% (shadowed gullies the sky test walks into)
        k = int(len(e) * 0.9)
        s = np.sort(np.abs(e))[:k]
        return float(np.sqrt(np.mean(s ** 2))), float(np.sqrt(np.mean(e ** 2))), m.sum()

def run(name, path, lat, lon, eye_off, hfov_hint, search_m=0.0, yaw_hint=None, yaw_slack=30.0, fov_slack=0.10, step_m=None):
    im = load(path)
    W, H = im.size
    sk = photo_skyline(im)
    xs = np.nonzero(sk >= 0)[0].astype(float)
    ys = sk[sk >= 0].astype(float)
    cx, cy = W / 2.0, H / 2.0
    # f in pixels from the 35 mm equivalent, assuming the full frame width is kept
    f_hint = (W / 2) / math.tan(math.radians(hfov_hint / 2))
    best = None
    offsets = [(0.0, 0.0)]
    if search_m:
        st = step_m or search_m / 2
        n = int(round(search_m / st))
        rng = [i * st for i in range(-n, n + 1)]
        offsets = [(dy, dx) for dy in rng for dx in rng]
    for dy, dx in offsets:
        la = lat + dy / 111320.0
        lo = lon + dx / (111320.0 * math.cos(math.radians(lat)))
        b, e = dem_skyline(la, lo, eye_off, 0.0, 359.9, 0.1, 30000.0)
        fit = Fitter(b, e, xs, ys, cx, cy)
        y0 = 0 if yaw_hint is None else yaw_hint - yaw_slack
        y1 = 360 if yaw_hint is None else yaw_hint + yaw_slack
        fit.bounds = [(y0 - 5, y1 + 5), (-20, 45), (-8, 8),
                      (f_hint / (1 + fov_slack), f_hint * (1 + fov_slack))]
        local = None
        for yaw in np.arange(y0, y1 + 0.1, 1.5):
            for fs in (1 / (1 + fov_slack), 1.0, 1 + fov_slack):
                for pitch in (-5, 0, 5, 10, 15, 20, 25):
                    p = np.array([yaw, pitch, 0.0, f_hint * fs])
                    c = fit.cost(p)
                    if local is None or c < local[0]: local = (c, p)
        for _ in range(3):
            r = minimize(fit.cost, local[1], method="Nelder-Mead",
                         options=dict(maxiter=4000, xatol=1e-4, fatol=1e-6))
            local = (r.fun, r.x)
        rob, raw, n = fit.rms(local[1])
        print(f"  {name} @ {la:.5f},{lo:.5f}: cost {local[0]:8.2f}  rms {rob:5.1f}px (raw {raw:5.1f})  "
              f"yaw {local[1][0] % 360:6.2f} pitch {local[1][1]:6.2f} roll {local[1][2]:5.2f} "
              f"hfov {2*math.degrees(math.atan(W/2/local[1][3])):5.1f} deg", flush=True)
        if best is None or local[0] < best[0]:
            best = (local[0], local[1], la, lo, b, e, rob, raw)
    return im, sk, xs, ys, cx, cy, best

def draw(im, best, cx, cy, out, colour=(255, 40, 40)):
    _, p, la, lo, b, e, rob, raw = best
    u, v = project(b, e, p[0], p[1], p[2], p[3], cx, cy)
    d = ImageDraw.Draw(im)
    pts = [(x, y) for x, y in zip(u, v) if np.isfinite(x) and np.isfinite(y) and -50 < x < im.width + 50]
    pts.sort()
    for i in range(1, len(pts)):
        if abs(pts[i][0] - pts[i-1][0]) < 12:
            d.line([pts[i-1], pts[i]], fill=colour, width=3)
    im.save(out)
