"""Readable results for the single-athlete long-ride pilot."""

import argparse
import base64
import csv
import json
from pathlib import Path
import shutil
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from long_data import OUTPUT, digest
from long_replay import BUDGETS, verify
from report import save_plot, table

ROOT = Path(__file__).resolve().parents[2]
CHECKPOINTS = (0, 10, 30, 60)
HISTORY_EDGES = (0, 50, 100, 200, 500, 1000, float("inf"))


def measure(rows):
    if not rows:
        return dict(n=0)
    actual = np.array([r["actual_minutes"] for r in rows])
    predicted = np.array([r["predicted_minutes"] for r in rows])
    low = np.array([r["low_minutes"] for r in rows])
    high = np.array([r["high_minutes"] for r in rows])
    error = predicted-actual
    return dict(n=len(rows), mape=float(np.mean(np.abs(error)/actual)*100),
        mae_minutes=float(np.mean(np.abs(error))), bias_minutes=float(np.mean(error)),
        median_ape=float(np.median(np.abs(error)/actual)*100),
        p90_ape=float(np.quantile(np.abs(error)/actual, .9)*100),
        coverage=float(np.mean((actual >= low) & (actual <= high))*100),
        above_upper=float(np.mean(actual > high)*100),
        median_width_minutes=float(np.median(high-low)),
        median_history_km=float(np.median([r["history_km"] for r in rows])),
        min_history_km=min(r["history_km"] for r in rows), max_history_km=max(r["history_km"] for r in rows))


def make_report(output, publish):
    protocol = verify(output)
    run = json.loads((output / "run.json").read_text())
    if digest(output / "predictions.csv") != run["predictions_sha256"]:
        raise ValueError("Prediction file changed")
    rows = []
    with (output / "predictions.csv").open() as f:
        for source in csv.DictReader(f):
            row = {k: v if k in ("mode", "ride", "year") or v == "" else float(v) for k, v in source.items()}
            rows.append(row)
    audit = json.loads((output / "audit.json").read_text())
    rides = json.loads((output / "rides.json").read_text())
    long_ids = {r["id"] for r in rides if r["moving_minutes"] >= 120}
    long_rows = [r for r in rows if r["ride"] in long_ids]

    def select(mode, checkpoint, budget=None):
        return [r for r in long_rows if r["mode"] == mode and r["checkpoint"] == checkpoint
                and (budget is None or r["history_target_km"] == budget)]

    summary = dict(audit=audit, run=run, protocol_sha256=digest(output / "protocol.json"),
        protocol={k: v for k, v in protocol.items() if k != "hashes"},
        years=sorted({r["date"][:4] for r in rides}),
        modes={mode: {str(p): measure(select(mode, p)) for p in CHECKPOINTS}
               for mode in ("continuous", "annual", "short_stops")},
        paired={str(b): {str(p): measure(select("paired", p, b)) for p in CHECKPOINTS} for b in BUDGETS})
    primary = summary["modes"]["continuous"]
    summary["annual_history"] = [dict(lower_km=lo, upper_km=None if np.isinf(hi) else hi,
        metrics=measure([r for r in select("annual", 0) if lo <= r["history_km"] < hi]))
        for lo, hi in zip(HISTORY_EDGES[:-1], HISTORY_EDGES[1:])]
    # Paired differences keep target identity, rather than comparing unrelated averages.
    cold = {r["ride"]: r for r in select("paired", 0, 0)}
    summary["paired_change"] = {}
    for budget in BUDGETS[1:]:
        trained = select("paired", 0, budget)
        delta = [100*(abs(r["predicted_minutes"]/r["actual_minutes"]-1)
                     - abs(cold[r["ride"]]["predicted_minutes"]/r["actual_minutes"]-1)) for r in trained]
        summary["paired_change"][str(budget)] = dict(mean_ape_change_pp=float(np.mean(delta)),
                                                     improved_rides=sum(d < 0 for d in delta), n=len(delta))
    longest = max(rides, key=lambda r: r["moving_minutes"])
    trajectory = [r for r in long_rows if r["mode"] == "continuous" and r["ride"] == longest["id"]]
    summary["longest"] = dict(moving_minutes=longest["moving_minutes"], distance_km=longest["distance_km"],
        checkpoints=[{k: r[k] for k in ("checkpoint", "at_minutes", "actual_minutes", "predicted_minutes", "low_minutes", "high_minutes")}
                     for r in trajectory if r["checkpoint"] in (0, 10, 30, 60, 120, 240, 480)])
    summary["without_longest"] = {str(p): measure([r for r in select("continuous", p) if r["ride"] != longest["id"]]) for p in CHECKPOINTS}

    plt.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "obc-long-v1", "font.size": 10,
                         "axes.spines.top": False, "axes.spines.right": False})
    colors = ("#305e4c", "#b8583f", "#708fa3", "#a47b28")
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.5))
    for p, color in zip(CHECKPOINTS, colors):
        values = [summary["paired"][str(b)][str(p)] for b in BUDGETS]
        label = "At departure" if p == 0 else f"After {p} moving min"
        axes[0].plot(range(len(BUDGETS)), [m["mape"] for m in values], "o-", label=label, color=color)
        axes[1].plot(range(len(BUDGETS)), [m["coverage"] for m in values], "o-", label=label, color=color)
    for ax in axes:
        ax.set(xticks=range(len(BUDGETS)), xticklabels=BUDGETS, xlabel="Minimum preceding history (km; whole rides)")
    axes[0].set(ylabel="Mean absolute remaining-time error (%)", title=f"Same {run['paired_target_rides']} long rides, one athlete", ylim=(0, None))
    axes[0].legend(frameon=False)
    axes[1].axhline(90, color="#777777", linestyle="--", label="Nominal 90%")
    axes[1].set(ylabel="Observed range coverage (%)", title="Ranges do not reach 90% consistently", ylim=(0, 105))
    axes[1].legend(frameon=False)
    save_plot(fig, output, "long-history")

    fig, ax = plt.subplots(figsize=(10, 4.5))
    at = np.array([r["at_minutes"] for r in trajectory])
    ax.plot(at/60, [(r["at_minutes"]+r["predicted_minutes"])/60 for r in trajectory], color=colors[0], label="Predicted finish")
    ax.fill_between(at/60, [(r["at_minutes"]+r["low_minutes"])/60 for r in trajectory],
                    [(r["at_minutes"]+r["high_minutes"])/60 for r in trajectory], color=colors[0], alpha=.15, label="Nominal 90% range")
    ax.axhline(longest["moving_minutes"]/60, color=colors[1], linestyle="--", label="Actual finish")
    ax.set(xlabel="Moving hours since departure", ylabel="Predicted total moving hours to finish",
           title="Longest retained ride: ETA changes despite substantial history")
    ax.legend(frameon=False)
    save_plot(fig, output, "long-trajectory")

    def metric_table(mode):
        return table(["Forecast", "Rides", "Mean error", "Mean absolute minutes", "Bias (min)", "Range coverage"],
            [("Departure" if p == 0 else f"After {p} min", mode[str(p)]["n"], f'{mode[str(p)]["mape"]:.1f}%',
              f'{mode[str(p)]["mae_minutes"]:.1f}', f'{mode[str(p)]["bias_minutes"]:+.1f}', f'{mode[str(p)]["coverage"]:.1f}%') for p in CHECKPOINTS])

    text = ["# Long rides after getting the computer\n",
        "## Result\n",
        f"The frozen estimator was replayed on **{audit['retained']} rides from one athlete**, with {audit['distance_km']:,.0f} km "
        f"of retained history. **{len(long_ids)} rides lasted at least two moving hours.** "
        "This is an external, single-athlete pilot. It does not establish accuracy for the user population or for MTB.\n",
        f"On the same {run['paired_target_rides']} long rides, mean departure error fell from "
        f"{summary['paired']['0']['0']['mape']:.1f}% with no history to {summary['paired']['100']['0']['mape']:.1f}% "
        "with at least 100 km of preceding rides. Live adjustment reduced the difference between history amounts. "
        "More history did not produce a steadily better estimate.\n",
        "**The long-day failure matters:** on the almost ten-hour ride, the estimate after ten minutes was more than "
        "three hours too optimistic, and the upper end of the range also missed the actual duration. "
        "Good average errors do not make this a reliable worst-case estimate.\n",
        "## Source and selection\n",
        f"We downloaded the [GoldenCheetah example archive]({audit['source']}). "
        f"It has {audit['source_summaries']} activity summaries and {audit['source_csvs']} sample files. "
        "The summaries include 592 cycling activities and 105 with at least two reported riding hours. "
        f"The retained recordings span {summary['years'][0]}–{summary['years'][-1]}. "
        "The main OSF file API timed out during this study; only the public GitHub example was used.\n",
        "We require an unambiguous metadata match, Bike sport, original GPS and altitude flags, finite samples, "
        "and at least five moving minutes and one kilometre. A whole ride is excluded for a recording gap over "
        "30 seconds, nonmonotonic time or distance, interval speed over 80 km/h, or a moving-interval gradient "
        "over 50%. We do not join usable sections across an unknown gap.\n",
        table(["First exclusion reason", "Sample files"],
            [(name.replace("_", " "), count) for name, count in sorted(audit["exclusions"].items())]),
        "Potential duplicate recordings on the same date, within 5% of both distance and elapsed time, are "
        "reduced to the finer recording. Overlapping retained activities are excluded. These checks are "
        "conservative heuristics; they cannot identify every duplicate or indoor/virtual ride. "
        "No surviving candidate was removed by the duplicate or overlap checks in this archive.\n",
        f"The final subset contains {audit['hours']['2']} rides of at least two hours, "
        f"{audit['hours']['4']} of at least four hours, and {audit['hours']['6']} of at least six hours. "
        "Thus the evidence mostly concerns two-to-four-hour rides. The strict screen can exclude real fast descents "
        "and steep pushing, as well as sensor faults. Longer recordings have more chances to fail a screen. "
        "These results apply to the retained subset, not all source rides.\n",
        "GPS coordinates are absent from the export. The original GPS flag indicates source-channel availability, "
        "not verified outdoor travel. The [GoldenCheetah source](https://github.com/GoldenCheetah/GoldenCheetah/blob/master/src/FileIO/RideFile.cpp) "
        "defines the flag. Bike categories are unavailable; every ride uses the frozen non-MTB default.\n",
        "## What the replay measures\n",
        "The core estimator, gradient curve, bike default, and reference error distributions are unchanged from "
        "the FitRec study. SHA-256 hashes freeze those inputs and this pilot's adapter before predictions are run. "
        "Shared parameters and reference ranges are not refitted on this athlete. Personal state still learns normally "
        "from earlier rides. The sample was inspected for suitability before the protocol was frozen.\n",
        "Distance increments and altitude provide an approximately 200 m trailing gradient. Positive-motion "
        "samples form roughly 20 m observation blocks, flushed at stops. Baseline block time is the sum of "
        "the sample distances times their default paces. Personal learning also requires at least 10 m and "
        "no more than two percentage points of gradient variation in a block.\n",
        "The outcome excludes zero-distance intervals. Slow positive movement, including pushing, has no minimum "
        "speed cutoff. There are no explicit pause events: distance rounding can remove slow movement, and "
        "GPS drift can turn a stop into apparent motion. This is a moving-time proxy, not exact ground truth.\n",
        "Forecasts use the full remaining recorded distance/elevation profile as a substitute for a planned route. "
        "Future riding times do not enter a forecast. This does not test map elevation errors or route deviations. "
        "A forecast is issued before the next observation block, approximately every ten moving minutes. "
        "The live multiplier applies to the entire remaining route.\n",
        "All eligible preceding rides can update personal pace. The first forecast in each phase/duration group "
        "can update the personal error buffer, only after its ride ends. This pilot queries full remaining routes; "
        "the earlier FitRec study queried fixed-distance targets. The reference error tables are not refitted.\n",
        "## Continuous new-device replay\n",
        "The simulated computer starts empty at the first retained ride and keeps its state through the archive. "
        "All rows below score the same long rides; the target is the time remaining at the stated checkpoint. "
        "Mean error is mean absolute percentage error. Negative bias means an estimate is too optimistic.\n",
        metric_table(primary),
        table(["Forecast", "90th percentile absolute error", "Median range width (min)"],
            [("Departure" if p == 0 else f"After {p} min", f"{primary[str(p)]['p90_ape']:.1f}%",
              f"{primary[str(p)]['median_width_minutes']:.1f}") for p in CHECKPOINTS]),
        "The range width is the upper estimate minus the lower estimate. Coverage must be read together with "
        "width: a broad range can cover more outcomes without giving a useful precise estimate.\n",
        "A missing or rejected source ride contributes no history. Recorded kilometres therefore mean retained "
        "device-observed distance, not this athlete's total riding. Fitness changes during unrecorded rides remain "
        "unobserved. The archive is not a complete account of the athlete's life.\n",
        "## Same long rides, different amounts of history\n",
        "For each target ride, start a fresh device before the latest whole preceding rides that reach the specified "
        "distance budget. Replay those rides in order, then score the target. The target itself never enters its "
        "own prior history. Only targets with at least 1,000 km of preceding retained history enter any paired row.\n",
        table(["Minimum history", "Actual km: median [min–max]", "Departure error", "After 10 min", "After 30 min", "After 60 min"],
            [(f"{b} km", f"{summary['paired'][str(b)]['0']['median_history_km']:.0f} "
              f"[{summary['paired'][str(b)]['0']['min_history_km']:.0f}–{summary['paired'][str(b)]['0']['max_history_km']:.0f}]",
              *[f"{summary['paired'][str(b)][str(p)]['mape']:.1f}%" for p in CHECKPOINTS]) for b in BUDGETS]),
        "Whole rides can overshoot a small distance budget substantially. A 50 km row must not be read as exactly "
        "50 km of learning. Different budgets also include different recent dates and numbers of ride-end updates.\n",
        "![History and range coverage on matched long rides](long-history.svg)\n",
        "The same targets make this a within-athlete comparison of history amounts. It is still descriptive: "
        "rides share training history, terrain, and one rider. We do not present a population confidence interval. "
        "Coverage is the fraction of outcomes inside the nominal 90% range, not a mathematical coverage guarantee.\n",
        "## Annual purchase dates\n",
        "A second replay resets the computer at the first retained ride of every calendar year. Each ride occurs "
        "once in this view. These are alternative purchase scenarios for the same athlete, not additional users.\n",
        metric_table(summary["modes"]["annual"]),
        table(["Prior retained distance", "Long rides", "Departure error"],
            [(f"{g['lower_km']}–{g['upper_km']} km" if g['upper_km'] else f"{g['lower_km']}+ km",
              g['metrics']['n'], f"{g['metrics']['mape']:.1f}%" if g['metrics']['n'] else "No observations")
             for g in summary["annual_history"]]),
        "These distance groups contain different target rides and can be small. They must not be interpreted "
        "as a controlled learning curve; the paired comparison above addresses that question.\n",
        "## The longest ride\n",
        f"The longest retained ride covers {longest['distance_km']:.1f} km in {longest['moving_minutes']/60:.2f} moving hours. "
        "It was selected for this diagnostic by duration, not by error. Each range below concerns remaining time.\n",
        table(["Moving minutes since start", "Actual remaining min", "Predicted remaining min", "Nominal 90% range (min)"],
            [(int(r['checkpoint']), f"{r['actual_minutes']:.0f}", f"{r['predicted_minutes']:.0f}",
              f"{r['low_minutes']:.0f}–{r['high_minutes']:.0f}") for r in summary['longest']['checkpoints']]),
        "![Predicted finish throughout the longest retained ride](long-trajectory.svg)\n",
        "The finish forecast changes substantially. The early live pace does not represent every later section. "
        "The single live multiplier cannot resolve all changes in terrain, effort, or conditions, and this archive "
        "does not identify their causes. Applying the multiplier to the full route is not a worst-case bound.\n",
        "## Stop-definition sensitivity\n",
        "The fixed sensitivity counts internal zero-distance runs of at most ten seconds as riding. Longer stops "
        "remain excluded. It uses the same source rides, model, and defaults, and replays their histories under "
        "the alternate definition. The target cohort remains the primary set of long rides.\n",
        metric_table(summary["modes"]["short_stops"]),
        f"Across all retained rides, this adds {audit['short_stop_minutes']:.1f} minutes "
        f"({100*audit['short_stop_minutes']/audit['moving_minutes']:.2f}% of primary moving time). "
        "This checks one ambiguity. It does not validate stop detection.\n",
        "## Decision and next evidence\n",
        "The pilot supports retaining the simple learner for further testing. A small amount of preceding "
        "history helps departure estimates for this athlete, while live adjustment supplies most of the "
        "in-ride adaptation. There is no basis here for more personal gradient or surface parameters.\n",
        "The uncertainty system remains unfinished. Its coverage varies with checkpoint and history, and it "
        "misses a large long-day error. Do not use these ranges as guaranteed upper bounds for water warnings.\n",
        "Next, obtain several riders with contiguous recorded histories, reliable bike labels, and multiple "
        "four-to-eight-hour rides. Original timestamped recordings and pause events are preferable. Include "
        "ordinary short rides in the histories. Reserve riders or later chronological periods before any "
        "new tuning; keep this pilot as inspected development evidence.\n",
        "## Reproducibility\n",
        f"Archive SHA-256: `{audit['source_sha256']}`. "
        f"Pilot protocol SHA-256: `{summary['protocol_sha256']}`. "
        f"The replay took {run['runtime_seconds']:.1f} seconds on this host; that is not an nRF54LM20 timing result.\n",
        "The host prototype README contains the exact preparation, freeze, replay, and report commands. "
        "Private artifacts retain per-ride predictions and prepared recordings. Public artifacts contain aggregate "
        "metrics and the selected diagnostic chart. No estimator parameters changed in this experiment.\n"]
    markdown = "\n".join(text)
    (output / "summary.json").write_text(json.dumps(summary, indent=2)+"\n")
    (output / "report.md").write_text(markdown)
    sys.path.insert(0, str(ROOT / "docs"))
    from build_docs import render_blocks
    rendered, _ = render_blocks(markdown)
    for name in ("long-history", "long-trajectory"):
        encoded = base64.b64encode((output / (name+".svg")).read_bytes()).decode()
        rendered = rendered.replace(f'src="{name}.svg"', f'src="data:image/svg+xml;base64,{encoded}"')
    (output / "report.html").write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>Long-ride pilot</title><style>body{max-width:1100px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;'
        'color:#243d32;background:#faf8f2}h2{margin-top:2em}img{max-width:100%}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 10px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}</style><body>'+rendered+'</body></html>')
    if publish:
        for name in ("long-history", "long-trajectory"):
            shutil.copyfile(output / (name+".svg"), ROOT / ("docs/assets/research/ride-time/"+name+".svg"))
            markdown = markdown.replace(f"]({name}.svg)", f"](/assets/research/ride-time/{name}.svg)")
        (ROOT / "docs/content/software/ride-time-long.md").write_text("---\ncopy: ai\n---\n\n"+markdown)
        (ROOT / "host/ride-time-prototype/results/long-v1.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(f"Report: {output / 'report.html'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--publish-docs", action="store_true")
    args = parser.parse_args()
    make_report(args.output, args.publish_docs)
