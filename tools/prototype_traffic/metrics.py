"""Plain error numbers for a traffic estimate: error factors, three levels and map-layer purity."""
import numpy as np, pandas as pd

LEVELS = (500.0, 2000.0)


def level(v):
    return np.searchsorted(LEVELS, v)


def table(pred, meas):
    """pred, meas: vehicles per day. Returns one row of plain numbers."""
    pred, meas = np.asarray(pred, float), np.asarray(meas, float)
    f = np.exp(np.abs(np.log(pred) - np.log(meas)))
    lp, lm = level(pred), level(meas)
    off = np.abs(lp - lm)
    near = np.min([np.abs(np.log(meas / b)) for b in LEVELS], axis=0) < np.log(1.25)
    g, r = lp == 0, lp == 2
    pct = lambda x: 100 * np.mean(x) if len(x) else np.nan
    return {"points": len(meas), "median factor": np.median(f), "within x1.5": pct(f <= 1.5), "within x2": pct(f <= 2),
            "within x3": pct(f <= 3), "beyond x5": pct(f > 5), "right level": pct(off == 0), "one off": pct(off == 1),
            "two off": pct(off == 2), "one-off near boundary": pct(near[off == 1]),
            "green: true <500": pct(meas[g] < 500), "green: true <1000": pct(meas[g] < 1000), "green: true >2000": pct(meas[g] > 2000),
            "red: true >2000": pct(meas[r] > 2000), "red: true >1000": pct(meas[r] > 1000), "red: true <500": pct(meas[r] < 500),
            "shown green": pct(g), "shown red": pct(r)}


def show(rows, title=""):
    df = pd.DataFrame(rows).T
    fmt = {c: ("{:.0f}" if c == "points" else "{:.2f}" if c == "median factor" else "{:.0f}") for c in df.columns}
    out = df.apply(lambda col: col.map(lambda v, c=col.name: fmt[c].format(v) if pd.notna(v) else "-"))
    return f"\n{title}\n" + out.to_string()
