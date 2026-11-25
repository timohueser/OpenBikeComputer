"""Draws minor roads of a 30 km square in three predicted traffic levels, with the measured count points on top."""
import sys, pickle
import numpy as np, pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection

feat, model, matched, lat, lon, title, out = sys.argv[1:8]
lat, lon = float(lat), float(lon)
F = pd.read_parquet(feat)
lon0, lat0 = np.load(feat.replace(".parquet", "_origin.npy"))
S = np.load(feat.replace(".parquet", "_segments.npz"))
cx, cy = (lon - lon0) * np.cos(np.radians(lat0)) * 111320, (lat - lat0) * 110574
inb = lambda x, y: (np.abs(x - cx) < 15000) & (np.abs(y - cy) < 15000)
F = F[inb(F.x, F.y)]
M = pickle.load(open(model, "rb"))
pred = np.exp(M["model"].predict(F[M["cols"]].values))
LEVELS, COLORS = [500, 2000], ["#43a047", "#ffa000", "#e53935"]
fig, ax = plt.subplots(figsize=(9, 9), dpi=100)
mid = (S["p0"] + S["p1"]) / 2
major = (S["cls"] % 10 <= 2) & inb(mid[:, 0], mid[:, 1])
ax.add_collection(LineCollection(np.stack([S["p0"][major], S["p1"][major]], 1), colors="#9e9e9e", linewidths=2.2))
lvl = np.searchsorted(LEVELS, pred)
for k in range(3):
    s = (lvl == k)
    ax.add_collection(LineCollection(np.stack([F[["x0", "y0"]].values[s], F[["x1", "y1"]].values[s]], 1), colors=COLORS[k], linewidths=0.5 + 0.7 * k))
m = pd.read_parquet(matched)
m = m[inb(m.x, m.y)]
ax.scatter(m.x, m.y, c=[COLORS[i] for i in np.searchsorted(LEVELS, m.aadt)], s=60, edgecolors="black", linewidths=1.1, zorder=5)
for x, y, v in zip(m.x, m.y, m.aadt):
    ax.annotate(f"{v:.0f}", (x, y), xytext=(4, 4), textcoords="offset points", fontsize=7)
ax.set_xlim(cx - 15000, cx + 15000); ax.set_ylim(cy - 15000, cy + 15000); ax.set_aspect("equal"); ax.set_xticks([]); ax.set_yticks([])
ax.set_title(f"{title}: minor roads by predicted motor vehicles per day (model fitted in England only).\n"
             "Green < 500, amber 500-2,000, red > 2,000. Dots: measured counts in the same colours. Grey: major roads. 30 km x 30 km.", fontsize=8)
fig.savefig(out, bbox_inches="tight")
print(out, "segments", len(F), "count points", len(m))
