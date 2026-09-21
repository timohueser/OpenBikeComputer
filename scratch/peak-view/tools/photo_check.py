"""Score the Peak View surface against photographs with a camera solved on 2 m LiDAR.

The camera for each photo is fitted to the *LiDAR* skyline, never to our own surface, so the
comparison cannot flatter us: both containers are then drawn through that one fixed camera.
"""
import math
import os
import pathlib
import subprocess
import sys

import numpy as np
from PIL import ImageDraw
from scipy.optimize import minimize

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from fit import load, photo_skyline, project
from overlay import Fitter

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")

SP = WORK
BIN = ROOT / "pano/target/release"
EYE = 1.6
# EXIF is stripped, so the lens is one of the iPhone's three, as a 35 mm equivalent.
LENSES = (13.0, 26.0, 48.0)


def cast_truth(fine_dir, coarse_dir, lat, lon, eye_m, b0, b1, step):
    """LiDAR where it reaches, GLO-30 beyond. The fourth column marks the LiDAR-borne columns."""
    out = subprocess.run([f"{BIN}/hybridsky", fine_dir, coarse_dir, f"{lat:.6f}", f"{lon:.6f}",
                          f"{eye_m:.2f}", f"{b0:.3f}", f"{b1:.3f}", f"{step}"],
                         capture_output=True, text=True, check=True).stdout
    a = np.array([[float(v) for v in line.split()] for line in out.splitlines()])
    return a[:, 0], a[:, 1], a[:, 3] > 0.5


def cast_obcd(obcd, lat, lon, b0, b1, step, far=40000.0):
    r = subprocess.run([f"{BIN}/obctsky", obcd, f"{lat:.6f}", f"{lon:.6f}", str(EYE),
                        f"{b0:.4f}", f"{b1:.4f}", str(step), str(far)], capture_output=True, text=True, check=True)
    a = np.array([[float(v) for v in line.split()] for line in r.stdout.splitlines()])
    return a[:, 0], a[:, 1], r.stderr.strip()


def solve(bearings, truth, xs, ys, W, H, yaw_hint, slack=45.0):
    """Yaw, pitch, roll and focal length that put the LiDAR skyline on the photo's."""
    best = None
    for lens in LENSES:
        f0 = (W / 2) / math.tan(math.radians(2 * math.degrees(math.atan(18.0 / lens)) / 2))
        fit = Fitter(bearings, truth, xs, ys, W / 2.0, H / 2.0)
        fit.bounds = [(yaw_hint - slack - 5, yaw_hint + slack + 5), (-25, 50), (-8, 8), (f0 / 1.08, f0 * 1.08)]
        seed = None
        for yaw in np.arange(yaw_hint - slack, yaw_hint + slack, 1.0):
            for fs in (1 / 1.08, 1.0, 1.08):
                for pitch in (-5, 0, 6, 12, 18, 24, 30):
                    p = np.array([yaw, pitch, 0.0, f0 * fs])
                    c = fit.cost(p)
                    if seed is None or c < seed[0]:
                        seed = (c, p)
        q = seed[1]
        for _ in range(3):
            q = minimize(fit.cost, q, method="Nelder-Mead",
                         options=dict(maxiter=5000, xatol=1e-4, fatol=1e-7)).x
        c = fit.cost(q)
        print(f"    lens {lens:4.0f} mm eq -> cost {c:9.2f}  yaw {q[0] % 360:6.2f} pitch {q[1]:6.2f} "
              f"hfov {2 * math.degrees(math.atan(W / 2 / q[3])):5.1f}", flush=True)
        if best is None or c < best[0]:
            best = (c, q, lens)
    return best


def angles(bearings, elevs, q, xs, ys, W, H):
    """Vertical error in degrees per detected column, NaN where the cast does not reach it.

    Every series keeps the full column grid so the three stay aligned; masking each one by its
    own coverage would give three different lengths.
    """
    pred = Fitter(bearings, elevs, xs, ys, W / 2.0, H / 2.0).predict(q)
    return np.degrees(np.arctan((pred - ys) / q[3]))


def run(case, dem_dir, coarse_dir, plain, crest):
    name, photo, (lat, lon), yaw_hint, floor, crop = case
    im = load(photo)
    W, H = im.size
    sk = photo_skyline(im, floor=floor)
    xs = np.nonzero(sk >= 0)[0].astype(float)
    ys = sk[sk >= 0].astype(float)
    ground = float(subprocess.run([f"{BIN}/demprobe", dem_dir, f"{lat:.6f}", f"{lon:.6f}"],
                                  capture_output=True, text=True, check=True).stdout.split()[0])
    print(f"\n### {name}  {lat:.5f},{lon:.5f}  LiDAR ground {ground:.1f} m, eye {ground + EYE:.1f} m")
    b0, b1 = yaw_hint - 70, yaw_hint + 70
    bear, truth, fine = cast_truth(dem_dir, coarse_dir, lat, lon, ground + EYE, b0, b1, 0.05)
    print(f"  {100 * fine.mean():.0f} % of the cast bearings find their horizon in the LiDAR")
    cost, q, lens = solve(bear, truth, xs, ys, W, H, yaw_hint)
    print(f"  camera: {lens:.0f} mm eq, yaw {q[0] % 360:.2f} pitch {q[1]:.2f} roll {q[2]:.2f} "
          f"hfov {2 * math.degrees(math.atan(W / 2 / q[3])):.1f} deg, cost {cost:.2f}")

    d = ImageDraw.Draw(im)
    series = {}
    for label, elevs, colour, note in [
            ("2 m LiDAR", truth, (0, 140, 255), ""),
            ("before", None, (255, 40, 40), plain),
            ("after", None, (0, 200, 60), crest)]:
        if elevs is None:
            _, elevs, err = cast_obcd(note, lat, lon, b0, b1, 0.05)
            print(f"  {label:10s} {err}")
        series[label] = angles(bear, elevs, q, xs, ys, W, H)
        u, v = project(bear, elevs, q[0], q[1], q[2], q[3], W / 2.0, H / 2.0)
        pts = sorted((x, y) for x, y in zip(u, v) if np.isfinite(x) and np.isfinite(y) and -50 < x < W + 50)
        for i in range(1, len(pts)):
            if abs(pts[i][0] - pts[i - 1][0]) < 12:
                d.line([pts[i - 1], pts[i]], fill=colour, width=3)

    # Score only where the truth agrees with the detected skyline: those columns are real terrain.
    trust = (np.abs(series["2 m LiDAR"]) < 0.5) & np.isfinite(series["before"]) & np.isfinite(series["after"])
    print(f"  -- {trust.sum()}/{len(trust)} columns the LiDAR confirms --")
    for label in ("before", "after"):
        e = series[label][trust]
        roll = np.convolve(e, np.ones(30) / 30, mode="valid")
        print(f"  {label:10s} median {np.median(e):+6.2f}  worst-30 {roll.max():+6.2f}  "
              f"rms {np.sqrt(np.mean(e ** 2)):5.2f} deg")
    im.save(f"{WORK}/pc_{name}.png")
    if crop:
        im.crop(crop).resize((960, round(960 * (crop[3] - crop[1]) / (crop[2] - crop[0])))).save(f"{WORK}/pc_{name}_zoom.png")
    return series, trust
