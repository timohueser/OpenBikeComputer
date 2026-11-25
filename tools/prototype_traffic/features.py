"""Builds the car graph of one region, assigns gravity trips between buildings and writes road features per segment."""
import sys, time
import numpy as np, pandas as pd
from numba import njit
from scipy.sparse import csr_matrix
from scipy.sparse.csgraph import dijkstra, connected_components
from scipy.spatial import cKDTree

npz, out = sys.argv[1:3]
SOURCES = int(sys.argv[3]) if len(sys.argv) > 3 else 1500
TAUS, LIMIT, NOISE = np.array([5.0, 15.0, 45.0]), 90.0, 0.25
FREE_KMH = np.array([105, 85, 70, 60, 50, 40, 25, 10], float)  # car speed by class; a tagged limit only lowers it
MINOR = [3, 4, 5, 6]
t0 = time.time()
log = lambda *a: print(f"[{time.time()-t0:6.1f}s]", *a, flush=True)
z = np.load(npz)
lat0, lon0 = np.median(z["lat"]), np.median(z["lon"])
xy = lambda lon, lat: np.c_[(np.asarray(lon) - lon0) * np.cos(np.radians(lat0)) * 111320, (np.asarray(lat) - lat0) * 110574]

# --- graph ---------------------------------------------------------------------------------
wn, ws = z["way_nodes"], z["way_start"]
craw = z["way_cls"].astype(int)
wcls, wlink = craw % 10, craw >= 10
uid, node = np.unique(wn, return_inverse=True)
P = np.zeros((len(uid), 2)); P[node] = xy(z["lon"], z["lat"])
seg_way = np.repeat(np.arange(len(ws) - 1), np.diff(ws) - 1)
last = np.zeros(len(wn), bool); last[ws[1:] - 1] = True
sa = np.flatnonzero(~last); a, b = node[sa], node[sa + 1]
seg_len = np.maximum(np.linalg.norm(P[a] - P[b], axis=1), 0.1)
deg = np.bincount(np.r_[a, b], minlength=len(uid))
first = np.zeros(len(sa), bool); first[np.r_[0, np.flatnonzero(np.diff(seg_way)) + 1]] = True
starts = first | (deg[a] != 2)
seg_edge = np.cumsum(starts) - 1
ne = seg_edge[-1] + 1
tag = z["way_speed"].astype(float)
kmh = np.where(tag > 0, np.minimum(FREE_KMH[wcls], 0.8 * tag), FREE_KMH[wcls]) * np.where(wlink, 0.8, 1)
e_len = np.bincount(seg_edge, seg_len, ne)
e_min = np.bincount(seg_edge, seg_len / (kmh[seg_way] / 3.6) / 60, ne)
si = np.flatnonzero(starts)
e_u, e_v, e_way = a[si], b[np.r_[si[1:] - 1, len(sa) - 1]], seg_way[si]
ow = z["way_oneway"][e_way]
U = np.r_[e_u[ow != -1], e_v[ow != 1]]; V = np.r_[e_v[ow != -1], e_u[ow != 1]]
E = np.r_[np.flatnonzero(ow != -1), np.flatnonzero(ow != 1)]
junc, inv = np.unique(np.r_[U, V], return_inverse=True)
U, V = inv[:len(U)], inv[len(U):]
W = e_min[E]
o = np.lexsort((W, V, U)); U, V, E, W = U[o], V[o], E[o], W[o]
k = np.r_[True, (np.diff(U) != 0) | (np.diff(V) != 0)]
U, V, E, W = U[k], V[k], E[k], np.maximum(W[k], 1e-4)
N = len(junc)
G = csr_matrix((W, (U, V)), shape=(N, N)); G.sort_indices()
Gi = csr_matrix((E + 1.0, (U, V)), shape=(N, N)); Gi.sort_indices()
assert (G.indices == Gi.indices).all()
_, comp = connected_components(G, directed=True, connection="weak")
mj = np.flatnonzero(comp == np.bincount(comp).argmax())
log(f"graph: {len(ws)-1} ways, {ne} edges, {N} junctions, {len(mj)} in the main part")

# --- trips -----------------------------------------------------------------------------------
bld = z["bld"]
bxy = xy(bld[:, 0], bld[:, 1])
jtree = cKDTree(P[junc][mj])
demand = np.bincount(mj[jtree.query(bxy)[1]], bld[:, 2], N)


@njit(cache=True)
def accumulate(order, pred, sub, indptr, indices, eid, flow):
    for n in order:
        p = pred[n]
        if p < 0:
            continue
        sub[p] += sub[n]
        for j in range(indptr[p], indptr[p + 1]):
            if indices[j] == n:
                flow[eid[j]] += sub[n]
                break


rng = np.random.default_rng(1)
nodes = np.flatnonzero(demand)
src = rng.choice(nodes, SOURCES, p=demand[nodes] / demand.sum())
flow = np.zeros((ne, len(TAUS)))
eid = (Gi.data - 1).astype(np.int64)
for i, s in enumerate(src):
    Gs = G.copy(); Gs.data = G.data * rng.lognormal(0, NOISE, G.nnz)  # spreads trips over near-equal paths
    dist, pred = dijkstra(Gs, indices=s, return_predecessors=True, limit=LIMIT)
    r = np.flatnonzero(np.isfinite(dist))
    sub = np.zeros((N, len(TAUS)))
    sub[r] = demand[r, None] * np.exp(-dist[r, None] / TAUS[None, :])
    accumulate(r[np.argsort(-dist[r])], pred, sub, G.indptr, G.indices, eid, flow)
    if i % 300 == 0:
        log(f"trips: source {i}/{SOURCES}")
flow *= demand.sum() / SOURCES
log("trips done")

# --- features of minor-road segments ---------------------------------------------------------
keep = np.flatnonzero(np.isin(wcls[seg_way], MINOR) & ~wlink[seg_way])
mid = (P[a] + P[b]) / 2
lo, hi = mid.min(0) - 3000, mid.max(0) + 3000
CELL = 100.0
shape = np.ceil((hi - lo) / CELL).astype(int) + 1


def box_sums(pts, w, radii):
    g = np.zeros(shape, np.float32)
    ij = ((pts - lo) / CELL).astype(int)
    ok = (ij >= 0).all(1) & (ij < shape).all(1)
    np.add.at(g, (ij[ok, 0], ij[ok, 1]), w[ok])
    S = np.pad(g.cumsum(0, dtype=np.float64).cumsum(1), ((1, 0), (1, 0)))
    q = ((mid[keep] - lo) / CELL).astype(int)
    res = []
    for r in radii:
        k_ = int(r / CELL)
        i0, j0 = np.clip(q[:, 0] - k_, 0, shape[0]), np.clip(q[:, 1] - k_, 0, shape[1])
        i1, j1 = np.clip(q[:, 0] + k_ + 1, 0, shape[0]), np.clip(q[:, 1] + k_ + 1, 0, shape[1])
        res.append(S[i1, j1] - S[i0, j1] - S[i1, j0] + S[i0, j0])
    del S, g
    return res


b_all = box_sums(bxy, bld[:, 2].astype(np.float32), [100, 300, 1000, 3000])
b_work = box_sums(bxy, (bld[:, 2] * (bld[:, 3] == 2)).astype(np.float32), [300, 1000])
heading = np.arctan2(*(P[b] - P[a]).T[::-1])
turn = np.zeros(len(sa))
same = np.r_[False, seg_edge[1:] == seg_edge[:-1]]
turn[same] = np.abs((np.diff(heading)[same[1:]] + np.pi) % (2 * np.pi) - np.pi)
e_bend = np.bincount(seg_edge, turn, ne) / np.maximum(e_len, 1) * 1000
pl = z["places"]
pxy = xy(pl[:, 0], pl[:, 1])
dist_place = lambda kinds: cKDTree(pxy[np.isin(pl[:, 2], kinds)]).query(mid[keep])[0]
major = np.flatnonzero(wcls[seg_way] <= 2)
nsub = np.maximum(1, np.ceil(seg_len[major] / 50)).astype(int)
mi = np.repeat(major, nsub)
fr = (np.arange(len(mi)) - np.repeat(np.cumsum(nsub) - nsub, nsub) + 0.5) / nsub[np.repeat(np.arange(len(major)), nsub)]
mdense = P[a[mi]] + (P[b[mi]] - P[a[mi]]) * fr[:, None]
e = seg_edge[keep]
w = seg_way[keep]
F = pd.DataFrame({
    "seg": keep, "x": mid[keep, 0], "y": mid[keep, 1], "x0": P[a[keep], 0], "y0": P[a[keep], 1], "x1": P[b[keep], 0], "y1": P[b[keep], 1],
    "way": z["way_id"][w], "ref": z["way_ref"][w], "cls": wcls[w], "tag_kmh": tag[w], "lanes": z["way_lanes"][w], "oneway": (z["way_oneway"][w] != 0) * 1,
    "width": z["way_width"][w], "marking": z["way_marking"][w], "surface": z["way_surface"][w], "lit": z["way_lit"][w],
    "edge_len": e_len[e], "bends_per_km": e_bend[e], "dead_end": ((deg[e_u[e]] == 1) | (deg[e_v[e]] == 1)) * 1,
    **{f"trips_{int(t)}min": np.log1p(flow[e, i]) for i, t in enumerate(TAUS)},
    **{f"bld_{r}m": np.log1p(v) for r, v in zip([100, 300, 1000, 3000], b_all)},
    **{f"work_{r}m": np.log1p(v) for r, v in zip([300, 1000], b_work)},
    "km_to_town": dist_place([0, 1]) / 1000, "km_to_village": dist_place([0, 1, 2, 4]) / 1000,
    "km_to_major": cKDTree(mdense).query(mid[keep])[0] / 1000 if len(mdense) else 50.0,
    "deg3_within_25m": 0,
})
F["deg3_within_25m"] = [len(x) > 0 for x in cKDTree(P[deg >= 3]).query_ball_point(mid[keep], 25.0)]
np.savez_compressed(out.replace(".parquet", "_segments.npz"), p0=P[a], p1=P[b], cls=wcls[seg_way] + 10 * wlink[seg_way], junctions=P[deg >= 3])
F.to_parquet(out)
np.save(out.replace(".parquet", "_origin.npy"), np.array([lon0, lat0]))
log(f"wrote {len(F)} minor-road segments")
