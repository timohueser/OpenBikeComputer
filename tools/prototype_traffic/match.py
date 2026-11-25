"""Matches count points to one minor-road segment and joins its features."""
import sys
import numpy as np, pandas as pd
from scipy.spatial import cKDTree

feat_path, counts_path, out = sys.argv[1:4]
F = pd.read_parquet(feat_path)
lon0, lat0 = np.load(feat_path.replace(".parquet", "_origin.npy"))
S = np.load(feat_path.replace(".parquet", "_segments.npz"))
c = pd.read_csv(counts_path)
cp = np.c_[(c.lon - lon0) * np.cos(np.radians(lat0)) * 111320, (c.lat - lat0) * 110574]
p0, p1, cls = S["p0"], S["p1"], S["cls"]
seg_len = np.linalg.norm(p1 - p0, axis=1)
near = np.flatnonzero(seg_len > 0)
nsub = np.maximum(1, np.ceil(seg_len[near] / 10)).astype(int)
si = np.repeat(near, nsub)
fr = (np.arange(len(si)) - np.repeat(np.cumsum(nsub) - nsub, nsub) + 0.5) / np.repeat(nsub, nsub)
dense = p0[si] + (p1[si] - p0[si]) * fr[:, None]
tree, jtree = cKDTree(dense), cKDTree(S["junctions"])
row_of = pd.Series(np.arange(len(F)), index=F.seg.values)
rows, why = [], {}
drop = lambda k: why.__setitem__(k, why.get(k, 0) + 1)
for i, p in enumerate(cp):
    hits = np.array(tree.query_ball_point(p, 25.0), int)
    if len(hits) == 0:
        drop("no road within 25 m"); continue
    if len(set(cls[si[hits]])) > 1:
        drop("two classes or a link within 25 m"); continue
    if jtree.query_ball_point(p, 25.0):
        drop("junction within 25 m"); continue
    s = si[hits[np.argmin(np.linalg.norm(dense[hits] - p, axis=1))]]
    if s not in row_of.index:
        drop("not a minor road"); continue
    r = F.iloc[row_of[s]]
    ref, cat, name = str(r.ref), c.cat.iloc[i], str(c.name.iloc[i])
    if (cat == "MB" and name not in ref.split(";")) or (cat == "MCU" and ref[:1] in ("A", "B", "M")):
        drop("road number differs"); continue
    rows.append((i, row_of[s]))
idx = np.array(rows)
M = pd.concat([c.iloc[idx[:, 0]].reset_index(drop=True), F.iloc[idx[:, 1]].reset_index(drop=True).drop(columns=["ref"])], axis=1)
M.to_parquet(out)
print(f"{len(c)} count points, {len(M)} matched; dropped {why}; by class {M.cls.value_counts().sort_index().to_dict()}")
