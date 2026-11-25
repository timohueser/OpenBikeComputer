"""Tests whether a car-trip score from the road network ranks minor-road traffic better than class, speed and lanes."""
import sys, time
import numpy as np, pandas as pd
from numba import njit
from scipy.sparse import csr_matrix
from scipy.sparse.csgraph import dijkstra, connected_components
from scipy.spatial import cKDTree
from scipy.stats import spearmanr, rankdata

osm_path, dft_path, out_dir = sys.argv[1:4]
REGION, SPLIT_LON = "East of England", 0.5
TAU_MIN, LIMIT_MIN, SOURCES = 15.0, 60.0, int(sys.argv[4]) if len(sys.argv) > 4 else 1000
CLASSES = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "living_street"]
MINOR = [3, 4, 5, 6]
FREE_KMH = np.array([105, 85, 70, 60, 50, 40, 25, 10], float)   # car travel speed by class, when no tag
ROUTER_KMH = np.array([90, 90, 70, 60, 50, 50, 30, 10], float)  # defaults of planner/router-build/src/traffic.rs
LAT0, LON0 = 52.2, 0.5
t0 = time.time()
log = lambda *a: print(f"[{time.time()-t0:6.1f}s]", *a, flush=True)


def xy(lon, lat):
    return np.c_[(np.asarray(lon) - LON0) * np.cos(np.radians(LAT0)) * 111320, (np.asarray(lat) - LAT0) * 110574]


# --- graph ---------------------------------------------------------------------------------
z = np.load(osm_path)
wn, ws = z["way_nodes"], z["way_start"]
cls_raw = z["way_cls"].astype(int)
way_cls, way_link = cls_raw % 10, cls_raw >= 10
uid, node = np.unique(wn, return_inverse=True)
P = np.zeros((len(uid), 2)); P[node] = xy(z["lon"], z["lat"])
nw = len(ws) - 1
seg_way = np.repeat(np.arange(nw), np.diff(ws) - 1)
last = np.zeros(len(wn), bool); last[ws[1:] - 1] = True
sa = np.flatnonzero(~last); a, b = node[sa], node[sa + 1]
seg_len = np.linalg.norm(P[a] - P[b], axis=1)
deg = np.bincount(np.r_[a, b], minlength=len(uid))
first = np.zeros(len(sa), bool); first[np.r_[0, np.flatnonzero(np.diff(seg_way)) + 1]] = True
starts = first | (deg[a] != 2)
seg_edge = np.cumsum(starts) - 1
ne = seg_edge[-1] + 1
tag_kmh = z["way_speed"].astype(float)
kmh = np.where(tag_kmh > 0, np.minimum(0.8 * tag_kmh, 1.25 * FREE_KMH[way_cls]), FREE_KMH[way_cls]) * np.where(way_link, 0.8, 1)
e_len = np.bincount(seg_edge, seg_len, ne)
e_min = np.bincount(seg_edge, seg_len / (kmh[seg_way] / 3.6) / 60, ne)
e_u = a[np.flatnonzero(starts)]
e_v = b[np.r_[np.flatnonzero(starts)[1:] - 1, len(sa) - 1]]
e_way = seg_way[np.flatnonzero(starts)]
ow = z["way_oneway"][e_way]
U = np.r_[e_u[ow != -1], e_v[ow != 1]]; V = np.r_[e_v[ow != -1], e_u[ow != 1]]
E = np.r_[np.flatnonzero(ow != -1), np.flatnonzero(ow != 1)]
junc, inv = np.unique(np.r_[U, V], return_inverse=True)
U, V = inv[:len(U)], inv[len(U):]
W = e_min[E]
o = np.lexsort((W, V, U)); U, V, E, W = U[o], V[o], E[o], W[o]
keep = np.r_[True, (np.diff(U) != 0) | (np.diff(V) != 0)]
U, V, E, W = U[keep], V[keep], E[keep], W[keep]
N = len(junc)
G = csr_matrix((np.maximum(W, 1e-6), (U, V)), shape=(N, N))
Gi = csr_matrix((E + 1.0, (U, V)), shape=(N, N))
Gi.sort_indices()
_, comp = connected_components(G, directed=True, connection="weak")
main = comp == np.bincount(comp).argmax()
log(f"graph: {nw} ways, {ne} edges, {N} junction nodes, {main.sum()} in main component")

# --- trips ---------------------------------------------------------------------------------
pl = z["places"]
DEFAULT_POP = np.array([100000, 15000, 600, 60, 4000, 5], float)
w = np.where(pl[:, 3] > 0, pl[:, 3], DEFAULT_POP[pl[:, 2].astype(int)])
JP = P[junc]
mj = np.flatnonzero(main)
d, k = cKDTree(JP[mj]).query(xy(pl[:, 0], pl[:, 1]))
ok = d < 2000
place_node, w = mj[k[ok]], w[ok]
demand_w = np.bincount(place_node, w, N)
log(f"places: {ok.sum()} snapped, total weight {w.sum():.0f}")


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


rng = np.random.default_rng(int(sys.argv[5]) if len(sys.argv) > 5 else 1)
indptr, indices, eid = Gi.indptr, Gi.indices, (Gi.data - 1).astype(np.int64)


def assign(demand):
    """Gravity trips from sources drawn by weight, each on its fastest car path; returns the flow of each edge."""
    nodes = np.flatnonzero(demand)
    src = rng.choice(nodes, SOURCES, p=demand[nodes] / demand.sum())
    flow = np.zeros(ne)
    for s in src:
        dist, pred = dijkstra(G, indices=s, return_predecessors=True, limit=LIMIT_MIN)
        reach = np.isfinite(dist)
        sub = np.where(reach, demand * np.exp(-np.where(reach, dist, 0) / TAU_MIN), 0.0)
        order = np.flatnonzero(reach)
        accumulate(order[np.argsort(-dist[order])], pred, sub, indptr, indices, eid, flow)
    return flow * demand.sum() / SOURCES


flow = assign(demand_w)
log("trips between places done")
bxy = xy(z["bld"][:, 0], z["bld"][:, 1])
flow_b = assign(np.bincount(mj[cKDTree(JP[mj]).query(bxy)[1]], minlength=N).astype(float))
log("trips between buildings done")

# --- count points ----------------------------------------------------------------------------
dft = pd.read_csv(dft_path, low_memory=False)
c = dft[(dft.road_type == "Minor") & (dft.estimation_method == "Counted") & (dft.year >= 2010) & (dft.region_name == REGION)]
c = c.sort_values("year").groupby("count_point_id").last().reset_index()
cp = xy(c.longitude, c.latitude)
step = 10.0
nsub = np.maximum(1, np.ceil(seg_len / step)).astype(int)
si = np.repeat(np.arange(len(sa)), nsub)
frac = (np.arange(len(si)) - np.repeat(np.cumsum(nsub) - nsub, nsub) + 0.5) / nsub[si]
dense = P[a[si]] + (P[b[si]] - P[a[si]]) * frac[:, None]
tree = cKDTree(dense)
jtree = cKDTree(P[deg >= 3])
refs = z["way_ref"]
rows, why = [], {}
for r, p in zip(c.itertuples(), cp):
    hits = tree.query_ball_point(p, 25.0)
    if not hits:
        why["no road within 25 m"] = why.get("no road within 25 m", 0) + 1; continue
    segs = np.unique(si[hits])
    ways = np.unique(seg_way[segs])
    if len(set(way_cls[ways])) > 1 or way_link[ways].any():
        why["two classes or a link"] = why.get("two classes or a link", 0) + 1; continue
    if jtree.query_ball_point(p, 25.0):
        why["junction within 25 m"] = why.get("junction within 25 m", 0) + 1; continue
    hits = np.array(hits)
    near = si[hits[np.argmin(np.linalg.norm(dense[hits] - p, axis=1))]]
    wy = seg_way[near]
    ref = refs[wy]
    if (r.road_category == "MB" and r.road_name not in ref.split(";")) or (r.road_category == "MCU" and ref[:1] in ("A", "B", "M")):
        why["road number differs"] = why.get("road number differs", 0) + 1; continue
    rows.append((r.count_point_id, r.longitude, r.latitude, r.year, r.road_category, r.road_name, r.all_motor_vehicles,
                 CLASSES[way_cls[wy]], way_cls[wy], tag_kmh[wy], z["way_lanes"][wy], z["way_id"][wy], seg_edge[near]))
m = pd.DataFrame(rows, columns="cp lon lat year cat name aadf highway cls tag_kmh lanes way edge".split())
log(f"count points: {len(c)} in region, {len(m)} matched; dropped {why}")
m.to_csv(f"{out_dir}/matched.csv", index=False)

# --- features ------------------------------------------------------------------------------
btree = cKDTree(bxy)


def features(cls, tag, lanes, edge, pts):
    speed = np.where(tag > 0, tag, ROUTER_KMH[cls])
    bld = np.array([len(x) for x in btree.query_ball_point(pts, 300.0)])
    return pd.DataFrame({"speed": speed, "tagged": (tag > 0).astype(float), "lanes2": (lanes >= 2).astype(float),
                         "through": np.log1p(flow[edge]), "through_b": np.log1p(flow_b[edge]), "local": np.log1p(bld)})


F = features(m.cls.values, m.tag_kmh.values, m.lanes.values, m.edge.values, xy(m.lon, m.lat))
m = pd.concat([m, F], axis=1)
m["y"] = np.log(m.aadf.clip(lower=1))
m["router_stress"] = (m.speed - 20).clip(lower=0) / 100 + m.cls.map({3: .1, 4: .05, 5: 0, 6: 0}) + 0.15 * m.lanes2
m.to_csv(f"{out_dir}/matched.csv", index=False)
MODELS = {"router rule (no fit)": None, "baseline fit: speed, tag, lanes": ["speed", "tagged", "lanes2"],
          "trip score alone": ["through"], "baseline + trip score": ["speed", "tagged", "lanes2", "through"],
          "baseline + trip score + buildings": ["speed", "tagged", "lanes2", "through", "local"],
          "buildings within 300 m alone": ["local"],
          "building trip score alone": ["through_b"],
          "baseline + building trip score": ["speed", "tagged", "lanes2", "through_b"]}


def fit_predict(tr, te, cols):
    if cols is None:
        return te.router_stress.values
    X = np.c_[np.ones(len(tr)), tr[cols].values]
    beta = np.linalg.lstsq(X, tr.y.values, rcond=None)[0]
    return np.c_[np.ones(len(te)), te[cols].values] @ beta


def auc(score, y):
    lo, hi = np.quantile(y, [1 / 3, 2 / 3])
    q, b_ = score[y <= lo], score[y >= hi]
    if len(q) == 0 or len(b_) == 0:
        return np.nan
    r = rankdata(np.r_[q, b_])
    return (r[len(q):].sum() - len(b_) * (len(b_) + 1) / 2) / (len(q) * len(b_))


def sp(s, y):
    return spearmanr(s, y).statistic if np.ptp(s) > 0 else 0.0


def ci(v):
    lo, hi = np.nanquantile(v, [0.025, 0.975])
    return f"[{lo:5.2f},{hi:5.2f}]"


report = []
for name, fit_east in (("fit west of 0.5°E, test east", False), ("fit east of 0.5°E, test west", True)):
    east = m.lon >= SPLIT_LON
    tr_all, te_all = (m[east], m[~east]) if fit_east else (m[~east], m[east])
    report.append(f"\n=== {name} ===")
    for c_ in MINOR:
        tr, te = tr_all[tr_all.cls == c_], te_all[te_all.cls == c_]
        y = te.y.values
        pred = {mn: fit_predict(tr, te, cols) for mn, cols in MODELS.items()}
        boots = rng.integers(0, len(y), (1000, len(y)))
        res = {mn: np.array([(sp(p[i], y[i]), auc(p[i], y[i])) for i in boots]) for mn, p in pred.items()}
        base = res["baseline fit: speed, tag, lanes"]
        report.append(f"\n{CLASSES[c_]}: fit {len(tr)} points, test {len(te)} points, test median AADF {te.aadf.median():.0f}")
        report.append(f"  {'model':36s} Spearman [95% CI]     AUC busy/quiet third   AUC minus baseline fit")
        for mn, p in pred.items():
            r = res[mn]
            report.append(f"  {mn:36s} {sp(p, y):5.2f} {ci(r[:, 0])}   {auc(p, y):4.2f} {ci(r[:, 1])}   {ci(r[:, 1] - base[:, 1])}")
txt = "\n".join(report)
print(txt)
open(f"{out_dir}/report.txt", "w").write(txt)
np.savez_compressed(f"{out_dir}/edges.npz", flow=flow, flow_b=flow_b, e_way=e_way, seg_edge=seg_edge, a=a, b=b, P=P, way_cls=way_cls, way_link=way_link,
                    tag=tag_kmh, lanes=z["way_lanes"])
log("done")
