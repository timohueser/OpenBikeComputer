"""Evidence and limitations of small long-ride model extensions."""

import argparse
import base64
import csv
import json
import math
from pathlib import Path
import shutil
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from endurance import MODES, OUTPUT, SOURCE, verify
from long_data import digest
from report import save_plot, table

ROOT = Path(__file__).resolve().parents[2]
LABELS = dict(baseline="Current model", gradient="Uphill correction", sustained="Sustained climb", duration="Duration correction")
PHASES = (0, 10, 30, 60)


def load(path):
    with path.open() as f:
        return [{k: v if k in ("mode", "ride") else float(v) for k, v in r.items()} for r in csv.DictReader(f)]


def metrics(rows):
    actual = np.array([r["actual_minutes"] for r in rows])
    prediction = np.array([r["predicted_minutes"] for r in rows])
    return dict(n=len(rows), mape=float(np.mean(np.abs(prediction/actual-1))*100),
                mae_minutes=float(np.mean(np.abs(prediction-actual))),
                bias_minutes=float(np.mean(prediction-actual)))


def report(output, publish):
    protocol = verify(output)
    run = json.loads((output / "run.json").read_text())
    for name in ("predictions", "states"):
        suffix = ".csv" if name == "predictions" else ".json"
        if digest(output / (name+suffix)) != run[name+"_sha256"]:
            raise ValueError("Changed run output: "+name)
    rows = load(output / "predictions.csv")
    states = json.loads((output / "states.json").read_text())
    rides = json.loads((SOURCE / "rides.json").read_text())
    longest = max(rides, key=lambda r: r["moving_minutes"])
    long_ids = {r["id"] for r in rides if r["moving_minutes"] >= 120}
    long_rows = [r for r in rows if r["ride"] in long_ids]
    key = lambda r: (r["ride"], int(r["checkpoint"]))
    original = {}
    with (SOURCE / "predictions.csv").open() as f:
        for row in csv.DictReader(f):
            if row["mode"] == "continuous":
                original[key(row)] = float(row["predicted_minutes"])
    current = {key(r): r["predicted_minutes"] for r in rows if r["mode"] == "baseline"}
    if current.keys() != original.keys():
        raise ValueError("Baseline targets differ from original pilot")
    baseline_difference = max(abs(current[k]/original[k]-1) for k in current)
    if baseline_difference > 1e-10:
        raise ValueError("Baseline predictions differ from original pilot")
    summary = dict(protocol={k: v for k, v in protocol.items() if k != "inputs"},
        protocol_sha256=digest(output / "protocol.json"), run=run,
        source_protocol_sha256=digest(SOURCE / "protocol.json"),
        baseline_check=dict(targets=len(current), maximum_relative_difference=baseline_difference),
        riders=1, rides=len(rides), long_rides=len(long_ids), modes={}, without_longest={}, support={}, longest={})
    for mode in MODES:
        select = lambda phase: [r for r in long_rows if r["mode"] == mode and r["checkpoint"] == phase]
        summary["modes"][mode] = {str(p): metrics(select(p)) for p in PHASES}
        summary["without_longest"][mode] = {str(p): metrics([r for r in select(p) if r["ride"] != longest["id"]]) for p in PHASES}
        own = [s for s in states if s["mode"] == mode]
        previous = [s for s in own if s["ride"] < longest["id"]]
        prediction = next(r for r in select(0) if r["ride"] == longest["id"])
        summary["support"][mode] = dict(coefficient_before_longest=prediction["coefficient"],
            multiplier_before_longest=math.exp(prediction["coefficient"]),
            identifiable_updates_before_longest=previous[-1]["identifiable_updates"],
            feature_km_before_longest=sum(s["feature_km"] for s in previous),
            saturated_km_before_longest=sum(s["saturated_km"] for s in previous),
            total_identifiable_updates=own[-1]["identifiable_updates"], final_coefficient=own[-1]["coefficient"],
            longest_share_of_centered_variance=(next(s["within_group_variance"] for s in own if s["ride"] == longest["id"])
                /sum(s["within_group_variance"] for s in own)) if sum(s["within_group_variance"] for s in own) else 0.)
        summary["longest"][mode] = [{k: r[k] for k in ("checkpoint", "predicted_minutes", "actual_minutes")}
            for r in long_rows if r["mode"] == mode and r["ride"] == longest["id"] and r["checkpoint"] in (*PHASES, 480)]
    summary["prior_maximum_minutes"] = max(r["moving_minutes"] for r in rides if r["date"] < longest["date"])

    plt.rcParams.update({"font.size": 11, "svg.fonttype": "none", "svg.hashsalt": "obc-endurance-v1",
                         "axes.spines.top": False, "axes.spines.right": False})
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.5))
    for mode, color in zip(MODES[1:], ("#305e4c", "#b8583f", "#708fa3")):
        for ax, group in zip(axes, ("modes", "without_longest")):
            delta = [summary[group][mode][str(p)]["mape"]-summary[group]["baseline"][str(p)]["mape"] for p in PHASES]
            ax.plot(range(4), delta, "o-", color=color, label=LABELS[mode])
    for ax, title in zip(axes, ("All 26 long rides", "Other 25 long rides")):
        ax.axhline(0, color="#666666", linestyle="--")
        ax.set(xticks=range(4), xticklabels=["Start", "10 min", "30 min", "60 min"], title=title,
               ylabel="Change in mean absolute error (percentage points)")
        ax.legend(frameon=False)
    save_plot(fig, output, "endurance-comparison")

    def comparison(group, field="mape"):
        return table(["Model", "Departure", "After 10 min", "After 30 min", "After 60 min"],
            [(LABELS[mode], *[f"{summary[group][mode][str(p)][field]:.2f}" + ("%" if field == "mape" else "") for p in PHASES]) for mode in MODES])

    support = summary["support"]
    text = ["# Small corrections for long rides\n", "## Result\n",
        "We compared three one-coefficient extensions with the frozen ride-time estimator. "
        "The replay uses the same **234 rides and 26 long rides from one athlete** as the long-ride pilot. "
        "This dataset was already inspected. These are exploratory results, not a new independent test.\n",
        "**The uphill correction gives a small benefit. The sustained-climb and duration corrections change "
        "little. None resolves the large error on the nine-hour ride.** The duration learner had little prior "
        "long-ride evidence and learned no positive decay before that ride.\n",
        "## Fixed hypotheses\n",
        table(["Variant", "Additional log-pace term"], [
            ("Uphill correction", "b × clip(gradient / 8%, 0, 1)"),
            ("Sustained climb", "b × uphill weight × clip(accumulated ascent / 300 m, 0, 1)"),
            ("Duration correction", "b × clip((moving minutes − 120) / 240, 0, 1)")]),
        "Each coefficient starts at zero. Gradient and sustained-climb coefficients are bounded by ±ln(1.5). "
        "The duration coefficient is bounded between zero and ln(1.5). At maximum feature exposure, this permits "
        "at most a 50% increase in pace, meaning minutes per kilometre. These are authored test settings, not "
        "physiological thresholds or values fitted on this rider. No nonzero population fatigue prior was available.\n",
        "Accumulated ascent increases on blocks above 2% gradient and resets after 200 m of non-climbing "
        "distance. Short interruptions do not reset it. It is a route-geometry proxy for a sustained climb, "
        "not an estimate of exhaustion or recovery.\n",
        "## Learning and causal forecasts\n",
        "Each variant retains its own original scalar learner and live multiplier. Persistent parameters remain "
        "fixed through the ride. At ride end, the added coefficient is fitted from the log-pace residual "
        "relative to the pre-ride scalar baseline. The scalar learner removes the pre-ride added correction "
        "from its observations. The live adjustment does not supply training targets.\n",
        "The gradient correction removes the ride mean from both feature and residual before regression. "
        "The other two corrections remove separate means within each ride and nearest gradient-anchor group. "
        "Thus a ride with flat terrain only early and climbing only late cannot by itself identify duration "
        "decay. Coarse gradient groups reduce, but do not remove, terrain and pacing confounding.\n",
        "For feature x, residual y, distance weight w, and group means x̄ and ȳ, calculate "
        "X = Σw(x−x̄)² and Y = Σw(x−x̄)(y−ȳ). Residuals are clipped to ±ln(2). "
        "Normalize X and Y by eligible ride distance W. With s = min(W/10 km, 1), "
        "update A ← ρA + 0.25sX/W and h ← ρh + 0.25sY/W, where ρ = 2^(−s/20). "
        "The candidate is h/(A+0.1), subject to the coefficient bounds and a maximum change of 0.03s. "
        "No evidence or decay is applied when X/W is below 10⁻⁶.\n",
        "A duration forecast starts from observed moving time so far. It walks through the remaining route, "
        "using predicted arrival time at each block to calculate future duration exposure. It never reads "
        "recorded future times for that calculation. Live observations divide by baseline time including "
        "the modeled current duration effect. The forecast therefore applies current fatigue once, then "
        "adds only the modeled future change.\n",
        "All variants use the same source screen, bike default, fixed gradient curve, observation blocks, "
        "and route-profile proxy. No parameter search or combined model was run. The freeze manifest predates "
        "these predictions. The baseline reproduces all original continuous-pilot forecasts within "
        f"{baseline_difference:.2g} relative difference.\n",
        "## Point accuracy on all long rides\n",
        "Mean absolute percentage error of full remaining moving time; each cell has 26 rides. "
        "These are the same target rides for every variant.\n", comparison("modes"),
        "Mean absolute error in minutes:\n", comparison("modes", "mae_minutes"),
        "![Error changes with and without the longest ride](endurance-comparison.svg)\n",
        "Negative changes mean lower error. The graph shows paired-cohort mean differences, not uncertainty "
        "bounds. All rides belong to one athlete and share histories. We do not bootstrap them as independent riders.\n",
        "## Does the largest error dominate the result?\n",
        "The following table excludes the longest ride from scoring. It stays in the chronological learning "
        "history for later rides. Each cell has 25 target rides.\n", comparison("without_longest"),
        "The uphill correction still gives a small improvement. This rules out the longest outcome as the "
        "sole source of the average benefit. It does not establish transfer to another rider.\n",
        "## What was learnable before the nine-hour ride?\n",
        table(["Variant", "Earlier identifiable ride updates", "Coefficient before ride", "Maximum pace multiplier"],
            [(LABELS[mode], support[mode]["identifiable_updates_before_longest"],
              f"{support[mode]['coefficient_before_longest']:.4f}",
              f"{support[mode]['multiplier_before_longest']:.3f}×") for mode in MODES[1:]]),
        f"The longest earlier retained ride lasted {summary['prior_maximum_minutes']/60:.2f} moving hours. "
        "No earlier ride reached the duration feature's six-hour saturation point. An identifiable update "
        "means the feature had some within-group variation; it does not mean that the ride supplied a strong "
        "or repeatable fatigue signal.\n",
        f"The duration correction had {support['duration']['identifiable_updates_before_longest']} earlier "
        "identifiable updates, but its constrained coefficient was zero when the long ride started. "
        "It consequently issued the same forecasts as the baseline on that ride. Learning from the completed "
        "ride cannot repair forecasts already issued.\n",
        "A shared nonzero duration prior could act before personal long-ride evidence exists. This experiment "
        "does not test one: its shape is shared, but its initial strength is zero. Choosing a population "
        "strength from this one failure would be tuning on the example we want to explain.\n",
        "## The nine-hour ride\n",
        table(["Moving minutes since departure", "Actual remaining", *[LABELS[m] for m in MODES]],
            [(int(p), f"{summary['longest']['baseline'][i]['actual_minutes']:.0f}",
              *[f"{summary['longest'][m][i]['predicted_minutes']:.0f}" for m in MODES])
             for i, p in enumerate((*PHASES, 480))]),
        "All values are minutes. The uphill correction moves the early estimate in the right direction, "
        "but the miss remains large. At some later checkpoints it is worse. A static correction does not "
        "represent the rider's reduction in climbing pace during this day.\n",
        "The previous broad gradient diagnostic found little median climbing-versus-flat discrepancy in "
        "37 qualifying earlier rides. The new coefficient uses a different sample and statistic: all "
        "eligible blocks, including rides with less than five minutes of steep climbing, with distance "
        "weights, ride centering, regularization, and chronological forgetting. A positive fitted coefficient "
        "is compatible with that near-zero median. Neither statistic alone establishes a stable rider trait.\n",
        "## Interpretation and device cost\n",
        "Keep the single uphill coefficient as a candidate for a new independent comparison. Its small gain "
        "on both the full cohort and the other long rides is more useful evidence than fitting a special curve "
        "to the nine-hour ride. It does not yet justify restoring a complete personal gradient curve.\n",
        "The near-zero duration result is inconclusive. Limited late-ride exposure, strong regularization, "
        "the chosen shared shape, and the zero prior all limit adaptation. We have not shown that fatigue "
        "is unimportant, that this shape is correct, or that a population prior would fail. The sustained-climb "
        "proxy is likewise only one simple hypothesis.\n",
        "The additional persistent fit needs three numbers: coefficient, curvature, and target. Grouped "
        "ride statistics can use five sums per gradient group, or 180 bytes for nine groups of float32 "
        "values. This Python experiment uses host arrays and does not measure device memory or timing.\n",
        "The duration variant has an additional cost beyond storing one coefficient: each forecast currently "
        "walks the remaining route to estimate future time. It cannot use the original constant-time remaining "
        "baseline sum. A device implementation would need a bounded route summary and a timing check. "
        "No firmware change is recommended from this pilot.\n",
        "## Next evidence\n",
        "Obtain multiple timestamped four-to-eight-hour rides per rider, with ordinary shorter rides between "
        "them. Include reliable bike labels and, where available, power data for diagnosis. Keep some riders "
        "or later rides untouched before fitting a shared duration prior. Compare the current model, the "
        "single uphill coefficient, and one constrained duration adjustment on that evidence.\n",
        "No new prediction-range claim is made. The old FitRec reference ranges do not establish coverage "
        "for these changed predictors; range calibration and validation would be separate work.\n",
        "## Reproduce the experiment\n",
        f"Protocol SHA-256: `{summary['protocol_sha256']}`. The four-model replay took "
        f"{run['runtime_seconds']:.1f} seconds on this host. The prototype README gives the exact freeze, "
        "run, and report commands. Per-ride predictions and coefficient histories remain in private artifacts. "
        "Public output contains aggregate results. The original estimator and previous experiment outputs "
        "remain unchanged.\n"]
    markdown = "\n".join(text)
    (output / "summary.json").write_text(json.dumps(summary, indent=2)+"\n")
    (output / "report.md").write_text(markdown)
    sys.path.insert(0, str(ROOT / "docs"))
    from build_docs import render_blocks
    rendered, _ = render_blocks(markdown)
    encoded = base64.b64encode((output / "endurance-comparison.svg").read_bytes()).decode()
    rendered = rendered.replace('src="endurance-comparison.svg"', f'src="data:image/svg+xml;base64,{encoded}"')
    (output / "report.html").write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>Long-ride model extensions</title><style>body{max-width:1100px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;'
        'color:#243d32;background:#faf8f2}h2{margin-top:2em}img{max-width:100%}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 10px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}</style><body>'+rendered+'</body></html>')
    if publish:
        shutil.copyfile(output / "endurance-comparison.svg", ROOT / "docs/assets/research/ride-time/endurance-comparison.svg")
        markdown = markdown.replace("](endurance-comparison.svg)", "](/assets/research/ride-time/endurance-comparison.svg)")
        (ROOT / "docs/content/software/ride-time-endurance.md").write_text("---\ncopy: ai\n---\n\n"+markdown)
        (ROOT / "host/ride-time-prototype/results/endurance-v1.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(f"Report: {output / 'report.html'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--publish-docs", action="store_true")
    args = parser.parse_args()
    report(args.output, args.publish_docs)
