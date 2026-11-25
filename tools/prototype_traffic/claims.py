"""Finds cuts on the predicted traffic so that "quiet" and "busy" claims are right 90 % or 95 % of the time, and reports them on held-out data."""
import os, sys, pickle
import numpy as np, pandas as pd
sys.path.insert(0, os.path.dirname(__file__))
from model import prep, gbm, cross_fit, blocks, SETS, CLASSES

eng = prep(pd.read_parquet(sys.argv[1]))
model = pickle.load(open(sys.argv[2], "rb"))
cols = SETS["road tags + places + trips"]
west, east = eng[eng.lon < 0.5], eng[eng.lon >= 0.5]
p_west = cross_fit(gbm(cols), west, blocks(west))


def cut(p, ok, target, low):
    """The widest cut whose claimed set is right at least `target` of the time; None if no cut reaches it."""
    order = np.argsort(p if low else -p)
    prec = np.cumsum(ok[order]) / np.arange(1, len(p) + 1)
    good = np.flatnonzero((prec >= target) & (np.arange(1, len(p) + 1) >= 20))
    return None if len(good) == 0 else p[order][good[-1]]


cuts = {}
for target in (0.90, 0.95):
    cuts[("quiet", target)] = cut(p_west, west.aadt.values < 1000, target, True)
    cuts[("busy", target)] = cut(p_west, west.aadt.values > 1000, target, False)
print("cuts on predicted vehicles per day, chosen on the west part of England:",
      {f"{k} {t:.0%}": (None if v is None else round(float(np.exp(v)))) for (k, t), v in cuts.items()})

sets = {"East England (model fitted on the west part)": (east, gbm(cols)(west, east))}
for path in sys.argv[3:]:
    o = prep(pd.read_parquet(path))
    name, p = path.split("/")[-1].replace("_matched.parquet", ""), model["model"].predict(o[cols].values)
    sets[name + " (England model, unchanged)"] = (o, p)
    sets[name + " (England model, one constant)"] = (o, p + np.median(o.y - p))
rows = []
for name, (o, p) in sets.items():
    for cls_name, sel in [("all", np.ones(len(o), bool))] + [(n, (o.cls == c).values) for c, n in CLASSES.items()]:
        if sel.sum() < 50:
            continue
        a, q = o.aadt.values[sel], p[sel]
        r = {"set": name, "class": cls_name, "points": sel.sum()}
        for (kind, t), v in cuts.items():
            if v is None:
                r[f"{kind} {t:.0%}: claimed"] = "no cut"; continue
            c = q <= v if kind == "quiet" else q >= v
            right = (a[c] < 1000) if kind == "quiet" else (a[c] > 1000)
            wrong = (a[c] > 2000) if kind == "quiet" else (a[c] < 500)
            r[f"{kind} {t:.0%}: claimed"] = f"{100 * c.mean():.0f} %"
            r[f"{kind} {t:.0%}: right"] = f"{100 * right.mean():.0f} %" if c.any() else "-"
            r[f"{kind} {t:.0%}: badly wrong"] = f"{100 * wrong.mean():.0f} %" if c.any() else "-"
        rows.append(r)
pd.set_option("display.width", 300); pd.set_option("display.max_columns", 30)
print(pd.DataFrame(rows).to_string(index=False))

# Claims that need no counts in the country: the quietest and busiest third by predicted value, within each class.
rows = []
for name, (o, p) in sets.items():
    if "one constant" in name:
        continue  # a constant does not change the order
    for cls_name, sel in [(n, (o.cls == c).values) for c, n in CLASSES.items()]:
        if sel.sum() < 50:
            continue
        a, q = o.aadt.values[sel], p[sel]
        ta, tq = np.searchsorted(np.quantile(a, [1 / 3, 2 / 3]), a, "right"), np.searchsorted(np.quantile(q, [1 / 3, 2 / 3]), q, "right")
        rows.append({"set": name, "class": cls_name, "points": sel.sum(),
                     "said quietest third: truly quietest": f"{100 * np.mean(ta[tq == 0] == 0):.0f} %",
                     "said quietest third: truly busiest": f"{100 * np.mean(ta[tq == 0] == 2):.0f} %",
                     "said busiest third: truly busiest": f"{100 * np.mean(ta[tq == 2] == 2):.0f} %",
                     "said busiest third: truly quietest": f"{100 * np.mean(ta[tq == 2] == 0):.0f} %"})
print(pd.DataFrame(rows).to_string(index=False))
