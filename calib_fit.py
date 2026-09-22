# Fit MapKit's snapshot camera: which perspective model reproduces snapshot.point(for:)?
import json, math, numpy as np
from scipy.optimize import least_squares
R = 6371000.0
def enu(lat, lon, lat0, lon0):
    return ((lon - lon0) * math.pi / 180 * R * math.cos(lat0 * math.pi / 180), (lat - lat0) * math.pi / 180 * R)
def project(E, N, U, d, pitch, head, F, W, H, cy=0.0):
    p, h = math.radians(pitch), math.radians(head)
    hd = np.array([math.sin(h), math.cos(h), 0.0]); z = np.array([0, 0, 1.0])
    C = -hd * d * math.sin(p) + z * d * math.cos(p)
    f = hd * math.sin(p) - z * math.cos(p)
    r = np.array([math.cos(h), -math.sin(h), 0.0]); u = np.cross(r, f)
    v = np.stack([E, N, U], -1) - C
    x, y, zz = v @ r, v @ u, v @ f
    return W / 2 + x / zz * F, H / 2 + cy - y / zz * F, zz
snaps = json.load(open("evidence/calib/calib.json"))
for s in snaps:
    P = np.array(s["pts"]); E, N = enu(P[:, 0], P[:, 1], s["lat"], s["lon"])
    ok = (np.abs(P[:, 2]) < 2000) & (np.abs(P[:, 3]) < 2000)
    def res(x):
        F, Z0, cy = x
        px, py, zz = project(E, N, np.full_like(E, Z0), s["dist"], s["pitch"], s["heading"], F, s["w"], s["h"], cy)
        m = ok & (zz > 1)
        return np.concatenate([(px - P[:, 2])[m], (py - P[:, 3])[m]])
    r = least_squares(res, [800, 0, 0])
    F, Z0, cy = r.x
    print(f"t={s['t']:.0f} pitch={s['pitch']:.0f} d={s['dist']:.0f} ele={s['ele']:.0f}  F={F:.1f}pt vfov={2*math.degrees(math.atan(s['h']/2/F)):.2f}deg  planeZ0={Z0:+.1f}m cy={cy:+.2f}  rms={np.sqrt(np.mean(r.fun**2)):.2f}px")

print("--- fixed vfov=30, fit pitch + distance (+Z0) at the steep snapshots")
for s in snaps:
    if s["pitch"] < 70: continue
    P = np.array(s["pts"]); E, N = enu(P[:, 0], P[:, 1], s["lat"], s["lon"])
    ok = (np.abs(P[:, 2]) < 2000) & (np.abs(P[:, 3]) < 2000)
    F = s["h"] / 2 / math.tan(math.radians(15))
    def res(x):
        pitch, d, Z0 = x
        px, py, zz = project(E, N, np.full_like(E, Z0), d, pitch, s["heading"], F, s["w"], s["h"])
        m = ok & (zz > 1)
        return np.concatenate([(px - P[:, 2])[m], (py - P[:, 3])[m]])
    r = least_squares(res, [60, s["dist"], 0])
    print(f"t={s['t']:.0f} req pitch=70 d={s['dist']:.0f} -> fitted pitch={r.x[0]:.2f} d={r.x[1]:.0f} Z0={r.x[2]:+.1f} rms={np.sqrt(np.mean(r.fun**2)):.2f}px")
