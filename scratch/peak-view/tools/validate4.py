import os
import pathlib
"""Solve each camera against the 2 m LiDAR, then score today's surface and the corrected one."""
import math, sys
import numpy as np, rasterio
from scipy.optimize import minimize
from PIL import ImageDraw
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from crestsim import build, Lattice, skyline, POST, ORIGIN
from overlay import Fitter
from fit import load, photo_skyline, project

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")
SP = WORK
U = str(ROOT / "photos")
LIFT_T, CONVEX_T = 20.0, 10.0

CASES = [
    ("hahnen",  f"{U}/hahnen.jpg", (46.80966, 8.42112, 1022.6), 58.3, 52.8,
     f"{WORK}/ne_alti.tif",   (20.0, 98.0),   (300, 320, 760, 580)),
    ("rigidal", f"{U}/rigidalstock-from-brunni.jpg", (46.80966, 8.42112, 1022.6), 10.4, 63.6,
     f"{WORK}/rigi_alti.tif", (-34.0, 56.0),  (150, 100, 750, 260)),
    ("urner",   f"{U}/urnerstaffel.jpg", (46.86881, 8.44342, 1697.0), 143.4, 63.6,
     f"{WORK}/urnr_alti.tif", (100.0, 190.0), (120, 60, 800, 280)),
]
base = build(f"{WORK}/dem/Copernicus_DSM_COG_10_N46_00_E008_00_DEM.tif",
             (46_740_000, 46_930_000), (8_340_000, 8_600_000))
g0 = base.g.copy()

def corrections(tif):
    ds = rasterio.open(tif); band = ds.read(1).astype(np.float32)
    if ds.nodata is not None: band[band == ds.nodata] = np.nan
    t = ds.transform
    lonp = t.c + (np.arange(band.shape[1]) + 0.5)*t.a; latp = t.f + (np.arange(band.shape[0]) + 0.5)*t.e
    ds.close()
    fy = (latp*1e6-ORIGIN)/POST - base.lat_idx[0]; fx = (lonp*1e6-ORIGIN)/POST - base.lon_idx[0]
    oy = (fy>=0)&(fy<g0.shape[0]-1.001); ox = (fx>=0)&(fx<g0.shape[1]-1.001)
    fy, fx, sub = fy[oy], fx[ox], band[np.ix_(oy,ox)]
    iy=fy.astype(int); ix=fx.astype(int); ty=(fy-iy)[:,None]; tx=(fx-ix)[None,:]
    surf = (g0[np.ix_(iy,ix)]*(1-tx)*(1-ty)+g0[np.ix_(iy,ix+1)]*tx*(1-ty)
            +g0[np.ix_(iy+1,ix)]*(1-tx)*ty+g0[np.ix_(iy+1,ix+1)]*tx*ty)
    resid = sub-surf
    ny=np.rint(fy).astype(np.int64); nx=np.rint(fx).astype(np.int64)
    flat=(np.repeat(ny[:,None],len(nx),1)*g0.shape[1]+np.repeat(nx[None,:],len(ny),0)).ravel()
    def smax(v):
        acc=np.full(g0.size,-np.inf,np.float32); r=v.ravel(); ok=np.isfinite(r)
        np.maximum.at(acc,flat[ok],r[ok]); return np.where(acc>-1e30,acc,np.nan).reshape(g0.shape)
    nodemax, lift = smax(sub), smax(resid)
    cov = np.isfinite(nodemax)
    filled = np.where(cov, nodemax, np.nan)
    p = np.pad(np.nan_to_num(filled, nan=np.nanmin(filled)), 1, mode="edge")
    lap = np.nan_to_num(filled, nan=0) - 0.25*(p[:-2,1:-1]+p[2:,1:-1]+p[1:-1,:-2]+p[1:-1,2:])
    return cov & (lift>LIFT_T) & (lap>CONVEX_T), nodemax, cov

def lidar_sampler(tif):
    ds = rasterio.open(tif); lb = ds.read(1).astype(np.float64)
    if ds.nodata is not None: lb[lb==ds.nodata]=np.nan
    T = ds.transform; ds.close()
    def f(la, lo):
        c,r = ~T*(lo,la); r=np.clip(r,0,lb.shape[0]-1.001); c=np.clip(c,0,lb.shape[1]-1.001)
        i0=r.astype(int);j0=c.astype(int);fr=r-i0;fc=c-j0
        return (lb[i0,j0]*(1-fc)*(1-fr)+lb[i0,j0+1]*fc*(1-fr)+lb[i0+1,j0]*(1-fc)*fr+lb[i0+1,j0+1]*fc*fr)
    return f

def robust(err, drop=0.05):
    a = np.sort(np.abs(err)); return float(np.sqrt(np.mean(a[:int(len(a)*(1-drop))]**2)))

for name, path, cam, yaw0, hfov0, tif, brange, crop in CASES:
    sel, nodemax, cov = corrections(tif)
    gc = g0.copy(); gc[sel] = np.maximum(gc[sel], nodemax[sel])
    im = load(path); W, H = im.size
    sk = photo_skyline(im); xs = np.nonzero(sk>=0)[0].astype(float); ys = sk[sk>=0].astype(float)
    f0 = (W/2)/math.tan(math.radians(hfov0/2))
    BEAR = np.arange(brange[0], brange[1], 0.04)
    lid = lidar_sampler(tif)
    a_lid, _ = skyline(lid, *cam, BEAR, far=9500.0, step=3.0)
    # solve the camera against the LiDAR skyline
    fit = Fitter(BEAR, a_lid, xs, ys, W/2.0, H/2.0)
    fit.bounds = [(yaw0-18, yaw0+18), (-25, 50), (-8, 8), (f0/1.14, f0*1.14)]
    best = None
    for y in np.arange(yaw0-14, yaw0+14, 0.75):
        for fs in (1/1.14, 1.0, 1.14):
            for p in (0, 8, 16, 24, 32):
                c = fit.cost(np.array([y, p, 0.0, f0*fs]))
                if best is None or c < best[0]: best = (c, np.array([y, p, 0.0, f0*fs]))
    q = best[1]
    for _ in range(3):
        q = minimize(fit.cost, q, method="Nelder-Mead", options=dict(maxiter=4000, xatol=1e-4, fatol=1e-6)).x
    print(f"\n{name}: {int(sel.sum())} overrides, {sel.sum()/(cov.sum()*57*39/1e6):.0f}/km2 | "
          f"camera solved on LiDAR: yaw {q[0]%360:.2f} pitch {q[1]:.2f} roll {q[2]:.2f} "
          f"hfov {2*math.degrees(math.atan(W/2/q[3])):.1f} (lens {hfov0})")
    d = ImageDraw.Draw(im)
    series = {}
    for sampler, colour, label, far in [
            (Lattice(base.lat_idx, base.lat, base.lon_idx, base.lon, g0).sample, (255,40,40), "today", 14000.0),
            (Lattice(base.lat_idx, base.lat, base.lon_idx, base.lon, gc).sample, (0,200,60), "corrected", 14000.0),
            (lid, (0,140,255), "2 m LiDAR", 9500.0)]:
        a, _ = skyline(sampler, *cam, BEAR, far=far, step=4.0)
        ft = Fitter(BEAR, a, xs, ys, W/2.0, H/2.0)
        pred = ft.predict(q); m = np.isfinite(pred)
        err = np.degrees(np.arctan((pred[m]-ys[m])/q[3]))
        r = np.convolve(err, np.ones(30)/30, mode="valid")
        series[label] = (err, m)
        print(f"   {label:11s} median {np.median(err):+.2f}  worst-30 {r.max():+.2f}  "
              f"robust rms {robust(err):.2f} deg")
        u, v = project(BEAR, a, q[0], q[1], q[2], q[3], W/2.0, H/2.0)
        pts = sorted((x,y) for x,y in zip(u,v) if np.isfinite(x) and np.isfinite(y) and -50<x<W+50)
        for i in range(1, len(pts)):
            if abs(pts[i][0]-pts[i-1][0]) < 12: d.line([pts[i-1], pts[i]], fill=colour, width=3)
    # score only where the LiDAR agrees with the detected skyline: those columns are real terrain
    trust = np.abs(series["2 m LiDAR"][0]) < 0.5
    print(f"   -- on the {trust.sum()}/{len(trust)} columns the LiDAR confirms --")
    for label in ("today", "corrected"):
        e = series[label][0][trust]
        rr = np.convolve(e, np.ones(30)/30, mode="valid")
        print(f"   {label:11s} median {np.median(e):+.2f}  worst-30 {rr.max():+.2f}  rms {np.sqrt(np.mean(e**2)):.2f} deg")
    im.save(f"{WORK}/v4_{name}.png")
    im.crop(crop).resize((940, round(940*(crop[3]-crop[1])/(crop[2]-crop[0])))).save(f"{WORK}/v4_{name}_zoom.png")
