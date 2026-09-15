"""Aggregate regional ETA comparison, with paired uncertainty across riders."""

import argparse
import base64
from collections import defaultdict
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

from enrichment_report import figure, table
from surface_data import OUTPUT, cohort, counts
from data import CACHE, valid_mask
from surface_model import MODELS, NAMES, features
from surface_replay import digest, identifier, rides, verify

ROOT = Path(__file__).resolve().parents[2]
LABELS = dict(gradient="Gradient", bike="+ bike category", path="+ path type", surface="+ surface")


def load(split):
    with (OUTPUT / f"{split}.csv").open() as f:
        rows = list(csv.DictReader(f))
    for row in rows:
        for field in ("phase", "target_km", "predicted_minutes", "actual_minutes", "surface_fraction", "path_fraction"):
            row[field] = float(row[field]) if row[field] else None
    keys = {name: {(r["rider"], r["ride"], r["phase"], r["target_km"], r["status"]) for r in rows if r["model"] == name}
            for name in MODELS}
    if any(value != keys["gradient"] for value in keys.values()):
        raise ValueError("Models do not share forecast targets and scoring decisions")
    return rows


def select(rows, phase, activity):
    return [r for r in rows if r["status"] == "scored" and r["phase"] == phase and
            (activity == "all" or (r["sport"] == "mountain bike") == (activity == "MTB"))]


def errors(rows, model):
    result = defaultdict(list)
    for row in rows:
        if row["model"] == model:
            result[row["rider"]].append(100*(row["predicted_minutes"]/row["actual_minutes"]-1))
    return result


def metrics(rows, model):
    grouped = errors(rows, model)
    if not grouped:
        return None
    flat = np.concatenate(list(grouped.values()))
    return dict(riders=len(grouped), targets=len(flat),
                mape=float(np.mean([np.mean(np.abs(v)) for v in grouped.values()])),
                bias=float(np.mean([np.mean(v) for v in grouped.values()])),
                median_ape=float(np.median(np.abs(flat))), p90_ape=float(np.quantile(np.abs(flat), .9)))


def paired(rows, candidate, reference):
    a, b = errors(rows, candidate), errors(rows, reference)
    if set(a) != set(b) or not a:
        raise ValueError("Paired comparison needs identical nonempty rider sets")
    delta = np.array([np.mean(np.abs(a[u]))-np.mean(np.abs(b[u])) for u in sorted(a)])
    rng = np.random.default_rng(20260915)
    draws = np.mean(rng.choice(delta, size=(20000, len(delta)), replace=True), axis=1)
    return dict(delta_pp=float(delta.mean()), bootstrap95_pp=np.quantile(draws, [.025, .975]).tolist(),
                riders=len(delta), improved_riders=int((delta < 0).sum()))


def support():
    result = {}
    for split in ("development", "calibration", "test"):
        result[split] = {}
        for activity in ("MTB", "other"):
            total = 0.
            tag_km = {key: defaultdict(float) for key in ("offline", "causal")}
            feature_km = np.zeros(len(NAMES))
            feature_users = [set() for _ in NAMES]
            for item, values, matched in rides(split):
                if (item["sport"] == "mountain bike") != (activity == "MTB"):
                    continue
                total += float(values[0].sum())
                for key, tags in matched.items():
                    for tag in ("surface", "highway"):
                        tag_km[key][tag] += sum(float(d)*sum(t.get(tag, {}).values()) for d, t in zip(values[0], tags))
                x = features(matched["causal"], item["sport"])
                feature_km += values[0].astype(float) @ x
                for i in range(len(NAMES)):
                    if np.any(x[:, i] > 0):
                        feature_users[i].add(item["user"])
            result[split][activity] = dict(raw_km=total, tag_km={k: dict(v) for k, v in tag_km.items()},
                feature_km=dict(zip(NAMES, feature_km.tolist())), feature_riders=dict(zip(NAMES, map(len, feature_users))))
    return result


def largest_change(rows):
    """Post-hoc inspection of the largest paired error change; no refit or replay."""
    selected = select(rows, 1, "all")
    a, b = errors(selected, "surface"), errors(selected, "bike")
    user = max(a, key=lambda u: np.mean(np.abs(a[u]))-np.mean(np.abs(b[u])))
    records = [r for r in selected if r["rider"] == user and r["model"] == "surface"]
    result = dict(targets=len(records), bike_mape=float(np.mean(np.abs(b[user]))),
                  surface_mape=float(np.mean(np.abs(a[user]))))
    if len({r["ride"] for r in records}) != 1:
        return result
    for item, values, matched in rides("test"):
        if identifier(item["ride"]) != records[0]["ride"]:
            continue
        valid = valid_mask(values)
        minutes, start = 0., 0
        for i in range(values.shape[1]):
            if minutes >= 10:
                start = i
                break
            if valid[i]:
                minutes += float(values[1, i])
        distance = np.r_[0., values[0].cumsum(dtype=float)]
        end = int(np.searchsorted(distance, distance[start]+10))
        if end >= len(distance):
            return result
        for name, key, indices in (("observed_prefix", "causal", np.flatnonzero(valid & (np.arange(values.shape[1]) < start))),
                                   ("future_10km", "offline", np.arange(start, end))):
            x = features(matched[key], item["sport"])[indices]
            w = values[0, indices].astype(float)
            w /= w.sum()
            result[name] = dict(path_fraction=float(w @ x[:, 3]),
                asphalt_fraction=sum(float(ww)*matched[key][i].get("surface", {}).get("asphalt", 0) for i, ww in zip(indices, w)))
        break
    return result


def report(publish):
    verify()
    fit = json.loads((OUTPUT / "fit.json").read_text())
    runtime = {split: json.loads((OUTPUT / f"{split}-runtime.json").read_text()) for split in ("calibration", "test")}
    splits = {split: load(split) for split in ("calibration", "test")}
    results = {}
    for split, rows in splits.items():
        results[split] = {}
        for phase in (0, 1):
            for activity in ("all", "MTB", "other"):
                group = select(rows, phase, activity)
                results[split][f"{phase}:{activity}"] = dict(
                    models={name: metrics(group, name) for name in MODELS},
                    surface_vs_bike=paired(group, "surface", "bike") if group else None,
                    surface_vs_path=paired(group, "surface", "path") if group else None)
    coverage = support()
    case = largest_change(splits["test"])
    primary = results["test"]["1:all"]["surface_vs_bike"]
    low, high = primary["bootstrap95_pp"]
    conclusion = ("The combined path and surface correction improves error in this regional test."
                  if high < 0 else "The combined path and surface correction makes error worse in this regional test."
                  if low > 0 else "This regional test does not show a clear improvement from the combined path and surface correction.")
    fig, axes = plt.subplots(1, 3, figsize=(12, 4), sharey=True)
    for ax, activity in zip(axes, ("all", "MTB", "other")):
        for phase, color in ((0, "#708fa3"), (1, "#305e4c")):
            values = [results["test"][f"{phase}:{activity}"]["models"][name]["mape"] for name in MODELS]
            ax.bar(np.arange(4)+(phase-.5)*.36, values, width=.36, color=color,
                   label="At start" if phase == 0 else "After 10 min")
        ax.set(title={"all": "All cycling", "MTB": "MTB", "other": "Other cycling"}[activity],
               xticks=np.arange(4), xticklabels=["Gradient", "+ bike", "+ path", "+ surface"])
        ax.tick_params(axis="x", rotation=30)
    axes[0].set_ylabel("Rider-weighted mean absolute error (%)")
    axes[0].legend()
    figure(fig, OUTPUT, "surface-accuracy")
    cohort_counts = counts(cohort())
    text = ["# Ride-time accuracy with OSM path and surface data\n",
        "**Status: experimental Python study.** The shared corrections are fitted on development riders. "
        "The final comparison uses the original test riders. It assumes the future route is known from the recorded track.\n",
        "## Main result\n", f"**{conclusion}**\n",
        f"After 10 accepted minutes, the primary comparison changes rider-weighted mean absolute percentage error "
        f"by **{primary['delta_pp']:+.2f} percentage points** versus the bike-category model. "
        f"The paired rider bootstrap interval is **{low:+.2f} to {high:+.2f} points** (95%). "
        f"Error falls for {primary['improved_riders']} of {primary['riders']} scored riders. Negative changes mean improvement.\n",
        "![Test-set error at start and after ten accepted minutes](surface-accuracy.svg)\n",
        "The test cohort contains only three MTB riders before forecast screening, and two supply scored live forecasts. MTB results are descriptive; "
        "many forecasts from three people do not create a large independent sample.\n",
        "## Data and separation\n",
        table(["Split", "Rides", "Riders", "MTB rides", "MTB riders"],
              [[s, c["rides"], c["riders"], c["mtb_rides"], c["mtb_riders"]] for s, c in cohort_counts.items()]),
        "Use every eligible ride fully contained in longitude 9–10° E and latitude 56–57° N. The region was chosen "
        "from development data for the earlier matching pilot. Keep the original rider hash split, first 60 candidate "
        "records and overlap exclusions. Only regional rides contribute to personal history in this study. "
        "The shared gradient curve comes from the original development-only fit. All four models also receive a "
        "development-fitted regional offset, so this is a new regional comparison, not a rerun of the original global benchmark.\n\n"
        "The previous global test results were already known. No regional calibration or test forecast errors were used "
        "to choose these coefficients or settings. Source code, match caches, cohort and fit were frozen before either "
        "cohort was replayed. Calibration results are a diagnostic comparison; they did not trigger tuning.\n",
        "## Test accuracy\n",
        "MAPE is the mean absolute percentage error. Average errors within each rider, then give riders equal weight. "
        "Signed bias uses the same weighting: positive means the estimate is too long. The 90th-percentile error is pooled "
        "over forecasts and gives frequent riders more weight. Each model receives exactly the same offered and scored targets "
        "within a phase. Start and live phases have different scored targets and riders, so their difference is not a "
        "paired estimate of the benefit from waiting ten minutes.\n"]
    for phase, title in ((0, "At the original ride start"), (1, "After ten accepted minutes")):
        text.append(f"### {title}\n")
        rows = []
        for activity in ("all", "MTB", "other"):
            for name in MODELS:
                m = results["test"][f"{phase}:{activity}"]["models"][name]
                rows.append([activity, LABELS[name], m["riders"], m["targets"], f"{m['mape']:.2f}%",
                             f"{m['bias']:+.2f}%", f"{m['p90_ape']:.2f}%"])
        text.append(table(["Activity", "Model", "Riders", "Targets", "MAPE", "Bias", "90th-percentile error"], rows))
    text += ["### Separate the surface contribution\n",
        table(["Activity", "Comparison after 10 min", "MAPE change", "95% paired rider bootstrap"], [
            [a, label, f"{m['delta_pp']:+.2f} pp", f"{m['bootstrap95_pp'][0]:+.2f} to {m['bootstrap95_pp'][1]:+.2f} pp"]
            for a in ("all", "MTB", "other") for key, label in
            (("surface_vs_bike", "Path + surface versus bike"), ("surface_vs_path", "Surface versus path"))
            for m in [results["test"][f"1:{a}"][key]]]),
        "These intervals resample riders with replacement 20,000 times, retaining each rider's paired mean errors. "
        "They describe uncertainty in the average comparison conditional on the fitted models and this region. "
        "They do not include training uncertainty or map errors. With only two MTB riders in the live comparison, bootstrap tails cannot "
        "describe the diversity of MTB users. Secondary and subgroup comparisons are exploratory, without correction "
        "for multiple comparisons. These are not ETA prediction intervals for an individual ride.\n",
        "### Post-hoc failure inspection\n",
        f"Most of the mean worsening comes from one rider with {case['targets']} scored live targets: error rises "
        f"from {case['bike_mape']:.2f}% to {case['surface_mape']:.2f}%. That rider remains in the primary result. "
        "This is a failure inspection selected after scoring, not a separate validation sample.\n\n"
        f"In the observed prefix, {100*case['observed_prefix']['path_fraction']:.0f}% of accepted distance carries the path-group feature "
        f"and {100*case['observed_prefix']['asphalt_fraction']:.0f}% has agreed asphalt. In the next 10 km, those figures are "
        f"{100*case['future_10km']['path_fraction']:.0f}% and {100*case['future_10km']['asphalt_fraction']:.0f}%. "
        "The shared model assigns a roughly 19% pace penalty to the path group, including paved paths. The live "
        "adjustment can absorb that excessive baseline penalty as unusually good rider form, then carry the speedup "
        "onto the following road. This is a plausible explanation for the failure, not proof of the historical "
        "road or surface. It identifies a risk of applying one path penalty across materials. No model was retuned "
        "and no rider was removed in response.\n",
        "### Calibration diagnostic\n",
        table(["Activity after 10 min", *LABELS.values()], [[a, *[f"{results['calibration'][f'1:{a}']['models'][m]['mape']:.2f}%"
                                                                  for m in MODELS]] for a in ("all", "MTB", "other")]),
        "## How much route information is available?\n",
        table(["Split", "Activity", "Recorded km", "Route-proxy surface", "Past-only surface", "Route-proxy highway", "Past-only highway"], [
            [s, a, f"{v['raw_km']:.0f}", *[f"{100*v['tag_km'][k][t]/v['raw_km']:.1f}%"
                                          for t in ("surface", "highway") for k in ("offline", "causal")]]
            for s, groups in coverage.items() for a, v in groups.items()]),
        "Percentages allocate agreed tag fractions to all original GPS chord distance, including intervals that fail "
        "the outcome screen. Unlike the earlier matching audit, feature extraction does not use measured speed to "
        "accept or reject a match. It retains the geometry, offset and gap checks. Therefore these figures have a "
        "different cohort and acceptance screen from the 80-ride audit. Attribute agreement remains conditional on "
        "the candidate paths considered; it does not verify physical material or historical map accuracy.\n",
        "## Model and safeguards\n",
        "For an interval, predicted pace is `exp(b(grade) + clip(x·beta, -ln(3), ln(3)) + personal + live)`. "
        "Here `b` is the fixed log-pace gradient curve. The shared features are a regional offset, an MTB indicator, "
        "three path fractions and four surface fractions. Forecast time is the sum of distance times predicted pace. "
        "The live factor applies to the entire remaining target. At ride start it is one.\n\n"
        "The models add features in this order: regional offset; bike category; track/path/cycleway; firm/soft/rough/generic "
        "unpaved surfaces. Path includes path, footway, bridleway and steps. Firm groups compacted, fine gravel, gravel "
        "and pebblestone. Soft groups ground, dirt, earth, grass, sand, mud and clay. Rough groups cobblestone, sett, "
        "unhewn cobblestone, rock and stone. Unknown and other values add zero correction; they are not declared paved. "
        "The map retains raw tags before grouping.\n\n"
        "Fractions are lower bounds common to the considered paths. They describe composition, not the position of "
        "each material. Weighting log corrections by these fractions is a simple approximation for mixed intervals; "
        "it is not an exact arithmetic sum over known surface subsegments. There are no gradient interactions, "
        "personal surface coefficients, trail-rating terms or weather inputs in this comparison.\n\n"
        "Fit the shared coefficients once with bounded ridge least squares. Clip observed log-pace residuals to ±ln(3). "
        "Within each learnable ride, weight observations by distance and normalize their total to one. Divide that "
        "weight by the rider's number of learnable rides. Center design and target within each ride, retaining half "
        "of the mean, so between-ride residuals have one quarter of the squared-error weight. Ridge strength is 0.25 "
        "per coefficient and 0.1 for the regional offset. Bound each coefficient to ±ln(2), and the offset to ±ln(1.5). "
        "Cap the combined correction to a factor from 1/3 to 3. No parameter sweep was performed.\n\n"
        "Each model then uses the original bounded scalar personal learner and five-minute live adjustment. Personal "
        "state stays fixed through the ride and updates at ride end. Past-only matching feeds both personal learning "
        "and live updates. Forecasts are issued before consuming the next observation.\n",
        "### Shared fitted multipliers\n",
        table(["Feature", *LABELS.values()], [[name, *[f"{np.exp(fit['coefficients'][m][i]):.3f}×" if i < len(fit['coefficients'][m]) else "—"
                                                      for m in MODELS]] for i, name in enumerate(NAMES)]),
        "These are fitted predictive associations, not physical speed penalties. Correlated bike, path and surface "
        "features can share the same effect. A coefficient near one can mean weak support rather than no real effect.\n",
        "## Forecast selection and practical limits\n"]
    target_rows = []
    for phase in (0, 1):
        for target in (1., 3., 10., 30.):
            offered = [r for r in splits["test"] if r["model"] == "bike" and r["phase"] == phase and r["target_km"] == target]
            scored = [r for r in offered if r["status"] == "scored"]
            target_rows.append(["Start" if phase == 0 else "After 10 min", int(target), len(offered), len(scored),
                                f"{np.median([r['actual_minutes'] for r in scored]):.1f}" if scored else "—"])
    text += [table(["Phase", "Target km", "Offered", "Scored", "Median actual min"], target_rows),
        "Targets are the first sample boundary at least 1, 3, 10 or 30 km ahead. Score a target only if every intervening "
        "interval passes the original screen: at most 30 seconds, at least 3 m displacement, 1–80 km/h, and slope within "
        "±50%. Personal learning additionally requires at least 10 m and adjacent slope change at most two percentage points. "
        "Rejected windows are not joined or replaced. The outcome is movement-screened elapsed time, not verified "
        "moving time. Hidden short stops remain, while some pushing and difficult terrain are excluded.\n\n"
        "Recorded future GPS geometry, altitude and offline tags substitute for a known planned route. No independent "
        "planned route exists here. Past-only matching prevents future coordinates from entering observation updates, "
        "but the route forecast remains retrospective. The current OSM snapshot can differ from the recorded roads "
        "and surfaces from 2009–2015. This is one lowland region, with few independent MTB test riders and limited "
        "long-horizon evidence. Recent rides with known routes and dense samples remain necessary.\n\n"
        "No individual-ride uncertainty bands were refitted in this small study. The previous global bands cannot be "
        "assumed calibrated after changing the model. This report assesses point accuracy and paired sampling uncertainty.\n",
        "## Device cost and next decision\n",
        f"The test replay took {runtime['test']['runtime_seconds']:.1f} seconds on the host, excluding map matching and fitting. "
        f"A scalar ride-end update took a median {1000*runtime['test']['finish_seconds_median']:.3f} ms "
        f"and a maximum {1000*runtime['test']['finish_seconds_max']:.3f} ms in this run. "
        f"Its persistent and current-ride NumPy arrays total {runtime['test']['scalar_array_bytes']} bytes. "
        "This excludes Python objects, route storage, the live state, prediction-range buffers and scratch memory.\n",
        "The largest shared model needs nine coefficients: 36 bytes as float32. The personal learner still has one "
        "parameter and fixed scalar sufficient statistics. Evaluating a segment adds at most nine products, a clamp "
        "and the existing exponential; it does not fit a shared model at ride end. The shared fit runs only on the host. "
        "The local OSM graph and Python matcher are research tools and do not meet the device memory budget. Route "
        "attribute storage and device map matching are separate unresolved integration work. Host array sizes and "
        "timings do not establish complete nRF54LM20 memory or execution time.\n\n"
        "Keep the simpler personal scalar and live adjustment as the implementation candidate. This run does not "
        "justify a blanket path penalty or the added material coefficients. The small surface-only change also does "
        "not establish that real surface conditions are unimportant. Preserve the enrichment pipeline and test recent "
        "rides with known surfaces, especially transitions between paved paths, roads and technical trails. A small "
        "combined road/surface classification is a candidate for that next independent experiment. Keep firmware "
        "integration and personal terrain parameters pending broader evidence.\n",
        "## Reproduction and sources\n",
        "The [prototype README](src:host/ride-time-prototype/README.md) gives the extraction, matching, freeze, replay "
        "and report commands. The portable report embeds its chart. Public artifacts contain aggregates; raw coordinates "
        "and per-forecast records remain private. [Earlier matching study](/docs/software/ride-time-matching/).\n\n"
        "Ride data: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html). Map data © "
        "[OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; extract supplied by Geofabrik. "
        "No ride coordinates were sent to a matching service.\n"]
    markdown = "\n".join(text)
    (OUTPUT / "report.md").write_text(markdown)
    sys.path.insert(0, str(ROOT / "docs"))
    from build_docs import render_blocks
    rendered, _ = render_blocks(markdown)
    encoded = base64.b64encode((OUTPUT / "surface-accuracy.svg").read_bytes()).decode()
    rendered = rendered.replace('src="surface-accuracy.svg"', f'src="data:image/svg+xml;base64,{encoded}"')
    rendered = rendered.replace('href="/docs/software/ride-time-matching/"', 'href="../ride-time-matching-v2/report.html"')
    (OUTPUT / "report.html").write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>Ride-time accuracy with OSM data</title><style>body{max-width:1100px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;'
        'color:#243d32;background:#faf8f2}h2{margin-top:2em}img{max-width:100%}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 10px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}</style><body>'+rendered+'</body></html>')
    source_audit = json.loads((CACHE / "fitrec.audit.json").read_text())
    aggregate = dict(cohort=cohort_counts, fit=fit, results=results, coverage=coverage, posthoc_case=case,
                     provenance=dict(fitrec={key: source_audit[key] for key in ("source_url", "source_sha256", "source_bytes")},
                         osm=json.loads(Path(".artifacts/ride-time-enrichment/osm-provenance.json").read_text()),
                         python=platform.python_version(), numpy=np.__version__, platform=platform.platform()),
                     manifest_sha256=digest(OUTPUT / "frozen.json"), targets=target_rows,
                     result_sha256={s: digest(OUTPUT / f"{s}.csv") for s in splits},
                     runtime=runtime)
    (OUTPUT / "summary.json").write_text(json.dumps(aggregate, indent=2)+"\n")
    if publish:
        shutil.copyfile(OUTPUT / "surface-accuracy.svg", ROOT / "docs/assets/research/ride-time/surface-accuracy.svg")
        public = markdown.replace("](surface-accuracy.svg)", "](/assets/research/ride-time/surface-accuracy.svg)")
        (ROOT / "docs/content/software/ride-time-surface.md").write_text("---\ncopy: ai\n---\n\n"+public)
        (ROOT / "host/ride-time-prototype/results/surface-v1.json").write_text(json.dumps(aggregate, indent=2)+"\n")
    print(conclusion)
    print(json.dumps(primary, indent=2))
    print(f"Report: {OUTPUT / 'report.html'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publish-docs", action="store_true")
    report(parser.parse_args().publish_docs)
