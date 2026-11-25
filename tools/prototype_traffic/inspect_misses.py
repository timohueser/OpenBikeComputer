"""Lists the worst misses of the building trip model and draws a 30 km map of its three levels."""
import sys
import numpy as np, pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection

out_dir, osm_npz = sys.argv[1:3]
CLASSES = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "living_street"]
ROUTER_KMH = np.array([90, 90, 70, 60, 50, 50, 30, 10], float)
COLS = ["speed", "tagged", "lanes2", "through_b"]
m = pd.read_csv(f"{out_dir}/matched.csv")
tr, te = m[m.lon < 0.5], m[m.lon >= 0.5]
beta = {}
for c in [3, 4, 5, 6]:
    t = tr[tr.cls == c]
    beta[c] = np.linalg.lstsq(np.c_[np.ones(len(t)), t[COLS].values], t.y.values, rcond=None)[0]
    print(CLASSES[c], "coefficients (intercept, speed km/h, speed tagged, 2+ lanes, log building trip flow):", np.round(beta[c], 3))
te = te.copy()
te["pred"] = [np.r_[1, r[COLS].values.astype(float)] @ beta[r.cls] for _, r in te.iterrows()]
te["miss"] = te.pred - te.y
w = te.reindex(te.miss.abs().sort_values(ascending=False).index).head(20)
pd.set_option("display.width", 220)
print(w.assign(pred_aadf=np.exp(w.pred).round(), speed=w.speed.round(), trip=np.expm1(w.through_b).round(), bld300=np.expm1(w.local).round())
      [["lat", "lon", "highway", "name", "year", "aadf", "pred_aadf", "speed", "trip", "bld300", "way"]].to_string(index=False))
w.way.to_csv(f"{out_dir}/miss_ways.txt", index=False, header=False)

# Map: 30 km x 30 km around Bury St Edmunds, in the test half.
e = np.load(f"{out_dir}/edges.npz")
z = np.load(osm_npz)
LAT0, LON0 = 52.2, 0.5
cx, cy = (0.715 - LON0) * np.cos(np.radians(LAT0)) * 111320, (52.245 - LAT0) * 110574
P, a, b, se = e["P"], e["a"], e["b"], e["seg_edge"]
seg_way = e["e_way"][se]
mid = (P[a] + P[b]) / 2
inb = (np.abs(mid[:, 0] - cx) < 15000) & (np.abs(mid[:, 1] - cy) < 15000)
cls = e["way_cls"][seg_way]
tag = e["tag"][seg_way]
speed = np.where(tag > 0, tag, ROUTER_KMH[cls])
X = np.c_[np.ones(len(se)), speed, tag > 0, e["lanes"][seg_way] >= 2, np.log1p(e["flow_b"][se])]
pred = np.full(len(se), np.nan)
for c in [3, 4, 5, 6]:
    s = cls == c
    pred[s] = np.exp(X[s] @ beta[c])
LEVELS, COLORS = [500, 2000], ["#4caf50", "#ffb300", "#e53935"]
fig, ax = plt.subplots(figsize=(11, 11), dpi=110)
major = inb & (cls <= 2)
ax.add_collection(LineCollection(np.stack([P[a[major]], P[b[major]]], 1), colors="#9e9e9e", linewidths=2.2))
for lvl in range(3):
    lo, hi = ([0] + LEVELS)[lvl], (LEVELS + [1e9])[lvl]
    s = inb & np.isin(cls, [3, 4, 5, 6]) & ~e["way_link"][seg_way] & (pred >= lo) & (pred < hi)
    ax.add_collection(LineCollection(np.stack([P[a[s]], P[b[s]]], 1), colors=COLORS[lvl], linewidths=0.6 + 0.6 * lvl))
pts = m[(np.abs((m.lon - LON0) * np.cos(np.radians(LAT0)) * 111320 - cx) < 15000) & (np.abs((m.lat - LAT0) * 110574 - cy) < 15000)]
px, py = (pts.lon - LON0) * np.cos(np.radians(LAT0)) * 111320, (pts.lat - LAT0) * 110574
ax.scatter(px, py, c=[COLORS[np.searchsorted(LEVELS, v)] for v in pts.aadf], s=70, edgecolors="black", linewidths=1.2, zorder=5)
for x_, y_, v in zip(px, py, pts.aadf):
    ax.annotate(f"{v:.0f}", (x_, y_), xytext=(4, 4), textcoords="offset points", fontsize=7)
ax.set_xlim(cx - 15000, cx + 15000); ax.set_ylim(cy - 15000, cy + 15000); ax.set_aspect("equal"); ax.set_xticks([]); ax.set_yticks([])
ax.set_title("Minor roads by predicted motor vehicles per day (green < 500, amber 500-2000, red > 2000).\n"
             "Dots: DfT counts, same colours, with the measured value. Grey: A roads and motorways. 30 km x 30 km around Bury St Edmunds.", fontsize=9)
fig.savefig(f"{out_dir}/map_bury.png", bbox_inches="tight")
lv = lambda v: np.searchsorted(LEVELS, v)
print("test half, three levels (rows: measured quiet/medium/busy, columns: predicted):")
print(pd.crosstab(lv(te.aadf.values), lv(np.exp(te.pred.values)), rownames=["measured"], colnames=["predicted"]))
