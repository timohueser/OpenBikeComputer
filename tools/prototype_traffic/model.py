"""Chooses a traffic model on one part of England, reports it on another part, and applies it unchanged to other countries."""
import os, sys, pickle
import numpy as np, pandas as pd
from sklearn.ensemble import HistGradientBoostingRegressor
from sklearn.model_selection import GroupKFold
sys.path.insert(0, __import__("os").path.dirname(__file__))
from metrics import table, show

pd.set_option("display.width", 250); pd.set_option("display.max_columns", 30)
CLASSES = {3: "secondary", 4: "tertiary", 5: "unclassified", 6: "residential"}
ROUTER_KMH = {3: 60, 4: 50, 5: 50, 6: 30}
TRIPS = ["trips_5min", "trips_15min", "trips_45min"]
PLACE = ["bld_100m", "bld_300m", "bld_1000m", "bld_3000m", "work_300m", "work_1000m", "km_to_town", "km_to_village", "km_to_major"]
ROAD = ["cls", "tag_kmh", "lanes", "oneway", "width", "marking", "surface", "lit", "edge_len", "bends_per_km", "dead_end"]
SETS = {"road tags only": ROAD, "road tags + places": ROAD + PLACE, "road tags + places + trips": ROAD + PLACE + TRIPS}


def prep(m):
    m = m[m.cls.isin(CLASSES) & (m.aadt > 0)].copy()
    m["y"] = np.log(m.aadt)
    m["speed"] = np.where(m.tag_kmh > 0, m.tag_kmh, m.cls.map(ROUTER_KMH))
    m["tagged"] = (m.tag_kmh > 0) * 1.0
    m["lanes2"] = (m.lanes >= 2) * 1.0
    return m


def class_only(tr, te):
    return te.cls.map(tr.groupby("cls").y.mean()).fillna(tr.y.mean()).values


def speed_lanes(tr, te):
    p = np.zeros(len(te))
    for c in CLASSES:
        a, b = tr[tr.cls == c], (te.cls == c).values
        if b.sum() == 0:
            continue
        X = lambda d: np.c_[np.ones(len(d)), d[["speed", "tagged", "lanes2"]].values]
        p[b] = X(te[b]) @ np.linalg.lstsq(X(a), a.y.values, rcond=None)[0]
    return p


def gbm(cols):
    def fit_predict(tr, te, keep=None):
        g = HistGradientBoostingRegressor(max_iter=300, learning_rate=0.04, max_leaf_nodes=15, min_samples_leaf=30,
                                          l2_regularization=1.0, categorical_features=[cols.index("cls")], random_state=0)
        g.fit(tr[cols].values, tr.y.values)
        if keep is not None:
            keep.append(g)
        return g.predict(te[cols].values)
    return fit_predict


MODELS = {"class only": class_only, "speed limit and lanes": speed_lanes, **{f"boosted trees: {k}": gbm(v) for k, v in SETS.items()}}


def rows(models, tr, te):
    return {name: table(np.exp(f(tr, te)), te.aadt.values) for name, f in models.items()}


def blocks(m, km=20):
    return (np.floor(m.x / (km * 1000)) * 1000 + np.floor(m.y / (km * 1000))).values


def cross_fit(f, m, groups, k=5):
    p = np.zeros(len(m))
    for a, b in GroupKFold(k).split(m, groups=groups):
        p[b] = f(m.iloc[a], m.iloc[b])
    return p


if __name__ == "__main__":
    eng = prep(pd.read_parquet(sys.argv[1]))
    west, east = eng[eng.lon < 0.5], eng[eng.lon >= 0.5]
    print(f"England: {len(eng)} matched points; choice part (west of 0.5° E) {len(west)}, report part (east) {len(east)}")
    if os.environ.get("REPORT", "1") == "1":
        cv = {n: table(np.exp(cross_fit(f, west, blocks(west))), west.aadt.values) for n, f in MODELS.items()}
        print(show(cv, "Choice: 5-fold cross-fit inside the west part, 20 km blocks"))
        rep = rows(MODELS, west, east)
        print(show(rep, "Report: fitted on the west part, tested on the east part (never seen by the choice)"))
        for c, n in CLASSES.items():
            e = east[east.cls == c]
            print(show(rows(MODELS, west, e), f"  report part, {n} only"))
    best = "boosted trees: road tags + places + trips"
    cols = SETS[best.split(": ")[1]]
    keep = []
    gbm(cols)(eng, eng.iloc[:1], keep)
    pickle.dump({"model": keep[0], "cols": cols}, open(sys.argv[2], "wb"))
    for path in sys.argv[3:]:
        o = prep(pd.read_parquet(path))
        name = path.split("/")[-1].replace("_matched.parquet", "")
        res = {}
        res["class only (England)"] = table(np.exp(class_only(eng, o)), o.aadt)
        res["speed limit and lanes (England)"] = table(np.exp(speed_lanes(eng, o)), o.aadt)
        p = keep[0].predict(o[cols].values)
        res["model (England), unchanged"] = table(np.exp(p), o.aadt)
        res["model (England), one constant adjusted"] = table(np.exp(p + np.median(o.y - p)), o.aadt)
        g = blocks(o, 20)
        res["class only, fitted in country"] = table(np.exp(cross_fit(class_only, o, g)), o.aadt)
        res["speed limit and lanes, fitted in country"] = table(np.exp(cross_fit(speed_lanes, o, g)), o.aadt)
        res["model fitted in country"] = table(np.exp(cross_fit(gbm(cols), o, g)), o.aadt)
        print(show(res, f"{name}: {len(o)} points (in country: 5-fold cross-fit, 20 km blocks); classes {o.cls.map(CLASSES).value_counts().to_dict()}"))
        adj = np.median(o.y - p)
        for c, n in CLASSES.items():
            b = (o.cls == c).values
            if b.sum() >= 50:
                print(show({"class only (England)": table(np.exp(class_only(eng, o[b])), o.aadt[b]),
                            "speed limit and lanes (England)": table(np.exp(speed_lanes(eng, o[b])), o.aadt[b]),
                            "model (England), unchanged": table(np.exp(p[b]), o.aadt[b]),
                            "model (England), one constant adjusted": table(np.exp(p[b] + adj), o.aadt[b])}, f"  {name}, {n} only"))
        print(f"  {name}: share of points with a trip score of 0: {np.mean(o.trips_15min == 0):.1%}; constant {adj:+.2f} (factor {np.exp(adj):.2f})")
        o.assign(pred=np.exp(p)).to_parquet(path.replace(".parquet", "_pred.parquet"))
