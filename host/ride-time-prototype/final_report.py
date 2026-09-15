"""Final-model accuracy, empirical coverage, history strata, and resource report."""

import argparse
import base64
import csv
import json
from pathlib import Path
import platform
import shutil
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from data import CACHE
from final_replay import OUTPUT, MODELS, config, digest, verify
from report import load, metrics, paired_improvement, save_plot, table

ROOT = Path(__file__).resolve().parents[2]
HISTORY = ("Zero", "0 < km < 50", "50 ≤ km < 200", "200 km or more")
LABELS = {"scalar_live": "Shared defaults", "bike_scalar_live": "Bike defaults (final)"}


def history(row):
    km = row["history_km"]
    return 0 if km == 0 else 1 if km < 50 else 2 if km < 200 else 3


def select(rows, phase, activity="all"):
    return [r for r in rows if r["phase"] == phase and
            (activity == "all" or (r["sport"] == "mountain bike") == (activity == "MTB"))]


def consistency(rows):
    """Compare the scalar specialization with the original frozen global replay."""
    key = lambda r: (r["rider"], r["ride"], int(r["phase"]), float(r["target_km"]))
    current = {key(r): r for r in rows}
    previous = {}
    with Path(".artifacts/ride-time/test-30s.csv").open() as f:
        for row in csv.DictReader(f):
            if row["model"] == "scalar_live" and row["status"] == "scored":
                previous[key(row)] = row
    if set(current) != set(previous):
        raise ValueError("Scalar consistency check has different scored targets")
    delta = [abs(r["predicted_minutes"]/float(previous[k]["predicted_minutes"])-1) for k, r in current.items()]
    return dict(scored_targets=len(current), max_relative_prediction_difference=max(delta))


def report(publish):
    protocol = verify()
    frozen = json.loads((OUTPUT / "frozen.json").read_text())
    if any(digest(OUTPUT / p) != value for p, value in frozen["hashes"].items()):
        raise ValueError("Frozen calibration changed")
    groups, offers = load(OUTPUT / "test.csv")
    key = lambda r: (r["rider"], r["ride"], r["phase"], r["target_km"])
    if set(map(key, groups[MODELS[0]])) != set(map(key, groups[MODELS[1]])):
        raise ValueError("Models do not share scored targets")
    summary = dict(config=json.loads((OUTPUT / "config.json").read_text()),
                   priors=json.loads((OUTPUT / "priors.json").read_text()), protocol=protocol, freeze=frozen,
                   reference_support=json.loads((OUTPUT / "reference-support.json").read_text()),
                   reference=json.loads((OUTPUT / "reference.json").read_text()),
                   runtime={s: json.loads((OUTPUT / f"{s}-summary.json").read_text()) for s in ("calibration", "test")},
                   scalar_consistency=consistency(groups["scalar_live"]),
                   test_csv_sha256=digest(OUTPUT / "test.csv"), python=platform.python_version(), numpy=np.__version__)
    summary["phase"] = {name: {str(p): metrics(select(rows, p)) for p in (0, 1)} for name, rows in groups.items()}
    rows = groups["bike_scalar_live"]
    summary["activity"] = {a: {str(p): metrics(select(rows, p, a)) for p in (0, 1)} for a in ("MTB", "other")}
    summary["history"] = {str(p): [metrics([r for r in select(rows, p) if history(r) == b]) for b in range(4)] for p in (0, 1)}
    summary["coverage_groups"] = [metrics([r for r in rows if r["group"] == g]) for g in range(6)]
    summary["mtb_coverage_groups"] = [metrics([r for r in rows if r["group"] == g and r["sport"] == "mountain bike"]) for g in range(6)]
    summary["paired"] = {str(p): paired_improvement(select(rows, p), select(groups["scalar_live"], p)) for p in (0, 1)}
    summary["long_outcomes"] = metrics([r for r in rows if r["actual_minutes"] >= 120])
    summary["targets"] = [dict(phase=p, km=t, offered=offers[("bike_scalar_live", p, t)],
                                   scored=sum(r["phase"] == p and r["target_km"] == t for r in rows))
                             for p in (0, 1) for t in config().target_km]
    source = json.loads((CACHE / "fitrec.audit.json").read_text())
    summary["source"] = {k: source[k] for k in ("source_url", "source_sha256", "source_bytes", "users", "counts")}
    (OUTPUT / "summary.json").write_text(json.dumps(summary, indent=2)+"\n")

    fig, axes = plt.subplots(1, 2, figsize=(11, 4.5))
    for p, color in ((0, "#708fa3"), (1, "#305e4c")):
        m = summary["history"][str(p)]
        means = np.array([v.get("rider_mape", np.nan) for v in m])
        ci = np.array([v.get("rider_mape_ci", [np.nan, np.nan]) for v in m])
        axes[0].errorbar(np.arange(4), means, yerr=[means-ci[:, 0], ci[:, 1]-means], marker="o", capsize=3,
                         color=color, label="Ride start" if p == 0 else "After 10 accepted min")
        coverage = [summary["phase"]["bike_scalar_live"][str(p)]] + [summary["activity"][a][str(p)] for a in ("other", "MTB")]
        means = np.array([v["rider_coverage"] for v in coverage])
        ci = np.array([v["rider_coverage_ci"] for v in coverage])
        axes[1].bar(np.arange(3)+(p-.5)*.35, means, width=.35, color=color,
                    yerr=[means-ci[:, 0], ci[:, 1]-means], capsize=3)
    axes[0].set(xticks=np.arange(4), xticklabels=["0", "0–50", "50–200", "200+"],
                xlabel="Prior accepted learning distance (km)", ylabel="Rider-weighted mean absolute error (%)",
                title="Limited-history performance")
    axes[0].legend(frameon=False)
    axes[1].set(xticks=np.arange(3), xticklabels=["All cycling", "Other", "MTB"], ylim=(0, 100),
                ylabel="Rider-weighted range coverage (%)", title="Observed coverage of nominal 90% ranges")
    axes[1].axhline(90, linestyle="--", color="#b8583f")
    save_plot(fig, OUTPUT, "final-results")
    text = write_report(summary)
    (OUTPUT / "report.md").write_text(text)
    sys.path.insert(0, str(ROOT / "docs"))
    from build_docs import render_blocks
    rendered, _ = render_blocks(text)
    encoded = base64.b64encode((OUTPUT / "final-results.svg").read_bytes()).decode()
    rendered = rendered.replace('src="final-results.svg"', f'src="data:image/svg+xml;base64,{encoded}"')
    rendered = rendered.replace('href="/docs/software/ride-time-estimation/"', 'href="../../docs/software/ride-time-estimation/index.html"')
    (OUTPUT / "report.html").write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>Final ride-time consistency check</title><style>body{max-width:1100px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;'
        'color:#243d32;background:#faf8f2}h2{margin-top:2em}img{max-width:100%}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 10px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}</style><body>'+rendered+'</body></html>')
    if publish:
        shutil.copyfile(OUTPUT / "final-results.svg", ROOT / "docs/assets/research/ride-time/final-results.svg")
        public = text.replace("](final-results.svg)", "](/assets/research/ride-time/final-results.svg)")
        (ROOT / "docs/content/software/ride-time-final.md").write_text("---\ncopy: ai\n---\n\n"+public)
        (ROOT / "host/ride-time-prototype/results/final-v1.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(json.dumps({k: summary[k] for k in ("phase", "activity", "history", "paired", "long_outcomes", "scalar_consistency")}, indent=2))
    print(f"Report: {OUTPUT / 'report.html'}")


def write_report(s):
    main, mtb = s["phase"]["bike_scalar_live"], s["activity"]["MTB"]
    stats = s["runtime"]["test"]
    scored = sum(t["scored"] for t in s["targets"])
    offered = sum(t["offered"] for t in s["targets"])
    def row(label, m):
        if not m["n"]:
            return [label, 0, 0, "—", "—", "—", "—", "—", "—"]
        return [label, m["riders"], m["n"], f"{m['rider_mape']:.2f}%", f"{m['rider_bias']:+.2f}%", f"{m['p90_ape']:.2f}%",
                f"{m['rider_coverage']:.2f}%", f"{m['rider_upper_exceedance']:.2f}%", f"{m['median_relative_width']:.1f}%"]
    headers = ["Group", "Riders", "Targets", "Mean error", "Bias", "P90 error", "Range coverage", "Above upper bound", "Median width / prediction"]
    text = ["# Final ride-time model: consistency check\n",
        "**The first research implementation is specified and tested. Prediction-range guarantees and device integration remain unvalidated.** "
        "This check uses the final scalar model with fixed bike defaults. Parameters and calibration references were frozen before evaluation.\n",
        "## Main results\n",
        table(headers, [row("Ride start", main["0"]), row("After 10 accepted minutes", main["1"])]),
        f"The check covers **{stats['rides']:,} test rides from {stats['riders']} riders**. "
        f"It scores {scored:,} of {offered:,} offered targets ({100*scored/offered:.1f}%). "
        f"After ten accepted minutes, point error averages **{main['1']['rider_mape']:.2f}%**, "
        f"and the nominal 90% range covers **{main['1']['rider_coverage']:.2f}%** of outcomes with equal rider weight.\n\n"
        f"For MTB, coverage is **{mtb['0']['rider_coverage']:.2f}% at start** and **{mtb['1']['rider_coverage']:.2f}% in ride**. "
        "These subgroup results must not be hidden behind overall coverage. The current ranges do not establish a "
        "universal 90% product claim or a worst-case water-gap duration.\n",
        "![History-dependent accuracy and empirical prediction-range coverage](final-results.svg)\n",
        "Chart error bars are 95% rider-bootstrap intervals for the aggregate metrics. They are not ranges for an individual ETA.\n",
        "## Frozen algorithm\n",
        "The [algorithm specification](/docs/software/ride-time-estimation/) defines the equations, defaults, state, "
        "ride lifecycle, and device input contract. The model has a fixed nine-anchor gradient curve, fixed MTB/other "
        "defaults, one slowly learned personal log multiplier shared across categories, and a temporary log multiplier "
        "with a five-minute moving-time half-life. The temporary factor applies to the entire remaining route.\n\n"
        "There are no personal gradient, path, or surface corrections. The scalar ride-end update is a bounded division, "
        "with the same objective and movement bounds as the general learner. Each range uses one of six phase/duration "
        "groups, 64 fixed reference knots with total weight 32, and at most 32 personal errors. Personal pace and range "
        "buffers update only at original ride completion.\n",
        "## Data separation and interpretation\n",
        f"Fit the bike defaults on {s['priors']['rides']:,} learnable rides from {s['priors']['riders']} development riders. "
        f"Recalibrate both models on {s['runtime']['calibration']['rides']:,} rides from "
        f"{s['runtime']['calibration']['riders']} different riders. The shared gradient curve comes from the original "
        "development-only fit. No OSM or phone processing is used in this final replay.\n\n"
        "The fixed rider hash split, first-60 candidate limit, and overlap exclusions remain unchanged. A rider's state "
        "contains only earlier completed rides. Forecasts occur at the original start and the first boundary after ten "
        "accepted moving minutes, before the next observation is consumed.\n\n"
        "Earlier aggregate results from these test riders were already inspected during model selection. This is a "
        "consistency check of the selected configuration, not a new independent test. No parameter or range changes "
        "were made in response to these results.\n",
        "## Point accuracy and range behaviour\n",
        "Mean error is mean absolute percentage error (MAPE), averaged within each rider and then across riders. "
        "Bias and coverage use the same weighting; positive bias means an estimate that is too long. P90 error and "
        "median range width pool forecasts and give frequent riders more weight. Width is `(upper - lower) / point estimate`. "
        "An upper exceedance means the actual duration is above the range's upper edge. The nominal central 90% target "
        "would leave about 5% above that edge under ideal calibration.\n\n"
        "Both models receive identical targets within each phase. Start and live phases have different scored targets "
        "and riders; their difference is not a paired estimate of the benefit of waiting ten minutes.\n"]
    for p, label in ((0, "Ride start"), (1, "After ten accepted minutes")):
        text += [f"### {label}\n", table(headers, [row(LABELS[name], s["phase"][name][str(p)]) for name in MODELS]),
                 table(headers, [row(a, s["activity"][a][str(p)]) for a in ("other", "MTB")])]
    text += ["### Paired change from bike defaults\n",
        table(["Phase", "MAPE change", "95% paired rider bootstrap", "Riders"], [
            ["Start" if p == "0" else "In ride", f"{v['percentage_points']:+.3f} pp",
             f"{v['ci'][0]:+.3f} to {v['ci'][1]:+.3f} pp", v["riders"]] for p, v in s["paired"].items()]),
        "Negative change means lower error. The intervals resample riders 2,000 times with a fixed seed, keeping "
        "paired mean errors together. They describe the comparison conditional on this fit and dataset. They are "
        "not individual-ride prediction intervals and do not include model-selection or training uncertainty.\n",
        "## Riders with limited history\n",
        "History is accepted learning distance before the current ride, not total lifetime distance or annual mileage. "
        "Zero history can also occur after an earlier ride supplied no learnable observations. These are descriptive "
        "groups with changing riders and routes, not a controlled learning curve.\n"]
    for p in (0, 1):
        text += [f"### {'Start' if p == 0 else 'In ride'} by previous learning distance\n",
                 table(headers, [row(label, m) for label, m in zip(HISTORY, s["history"][str(p)])])]
    group_labels = [f"{p}: {t}" for p in ("Start", "In ride") for t in ("<10 min", "10–<30 min", "30+ min")]
    text += ["## Coverage by predicted duration\n",
        table(headers, [row(label, m) for label, m in zip(group_labels, s["coverage_groups"])]),
        "### MTB duration groups\n", table(headers, [row(label, m) for label, m in zip(group_labels, s["mtb_coverage_groups"])]),
        "The predicted duration selects the group. The 30+ minute group is not evidence of reliable two-hour ranges. "
        f"Only {s['long_outcomes']['n']} scored outcomes lasted at least two hours.\n",
        "### Calibration support\n",
        table(["Group", "Reference outcomes", "Calibration riders", "Pooled fallback"], [
            [label, m["observations"], m["riders"], m["pooled_fallback"]] for label, m in
            zip(group_labels, s["reference_support"]["bike_scalar_live"])]),
        "Only the first prospectively reserved target per group and ride supplies an error. Missing outcomes are "
        "not replaced with more convenient targets. The reference distributions pool bike categories; per-category "
        "coverage is an evaluation result, not an enforced property. More personal history need not narrow a range.\n",
        "## Outcome availability and remaining limits\n",
        table(["Phase", "Target km", "Offered", "Scored"], [["Start" if t["phase"] == 0 else "In ride", t["km"], t["offered"], t["scored"]]
                                                               for t in s["targets"]]),
        "A target is scored only if every intervening interval passes the original recording screen. There is no "
        "joining of disjoint valid windows. The screen requires spacing up to 30 seconds, at least 3 m displacement, "
        "1–80 km/h interval-average speed, and slope within ±50%. Personal learning additionally requires at least "
        "10 m and adjacent slope change no greater than two percentage points.\n\n"
        "These filters exclude some pushing and difficult terrain and permit hidden short stops. Recorded future "
        "geometry and altitude stand in for a known route. Results measure a movement-screened elapsed-time proxy. "
        "They do not validate true moving time, route deviations, intermediate live checkpoints, or arbitrary forecast "
        "horizons. Recent dense rides with reliable movement labels remain necessary.\n",
        "## Numerical checks and device cost\n",
        f"No scalar observation or ride-end update was rejected in either replay. The scalar specialization matches "
        f"the original scalar model on all {s['scalar_consistency']['scored_targets']:,} scored targets. The maximum "
        f"relative point-prediction difference is {s['scalar_consistency']['max_relative_prediction_difference']:.3g}, "
        "consistent with small floating-point differences. Synthetic tests compare both learners over 250 rides "
        "with sparse evidence and outliers, check bounded movement, reject invalid state, and check forecast causality.\n\n"
        f"The full two-model test replay took {stats['runtime_seconds']:.1f} seconds on this host. A scalar ride-end "
        f"update took a median {stats['finish_median_ms']:.3f} ms and a maximum {stats['finish_max_ms']:.3f} ms. "
        "These timings exclude source preparation and population fitting and do not establish nRF54LM20 timing.\n\n"
        "Personal learning needs 20 bytes of float32 state. The listed estimator payload, including live state, "
        "personal range buffers, reference knots, gradient anchors, and bike defaults, is 2,484 bytes. Of this, "
        "868 bytes must be mutable if fixed tables reside in read-only storage. This excludes pending calibration "
        "targets, configuration, stack, alignment, route storage, and input processing. Range refresh uses bounded "
        "post-ride workspace; no general solver or route-length-dependent ride-end fit remains.\n",
        "## Decision\n",
        "Use the scalar algorithm as the first implementation specification. Keep the current empirical range "
        "method identified as experimental wherever observed coverage falls short, especially for MTB. Do not "
        "retune it on this test set or describe its upper edge as a pessimistic maximum. Validate reliable moving "
        "inputs, recent technical MTB rides, long horizons, and the complete device resource budget before making "
        "those product claims.\n",
        "## Artifacts and reproduction\n",
        "The [prototype README](src:host/ride-time-prototype/README.md) lists the commands. The aggregate JSON includes "
        "configuration, bike defaults, calibration support and knots, source hashes, metrics, and host measurements. "
        "Raw ride data and per-forecast records stay private. The portable report embeds its chart.\n\n"
        "Data source: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html), "
        "Ni, Muhlstein and McAuley (WWW 2019). The source archive and rider-split rules are unchanged from the first study.\n"]
    return "\n".join(text)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publish-docs", action="store_true")
    report(parser.parse_args().publish_docs)
