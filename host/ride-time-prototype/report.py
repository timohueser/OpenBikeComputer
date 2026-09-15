"""Build an aggregate report and portable HTML without publishing ride records."""

import argparse
import base64
from collections import Counter, defaultdict
import csv
import hashlib
import json
from pathlib import Path
import platform
import shutil
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import scipy

from data import CACHE
from replay import MODELS

ROOT = Path(__file__).resolve().parents[2]
NAMES = dict(zip(MODELS, ("Fixed pace", "Fixed gradient", "Personal scale", "Personal gradient", "Scale + live", "Gradient + live")))
COLORS = ("#8d9c90", "#315e4b", "#bd572e")
BOOTSTRAPS = 2000


def load(path):
    groups, offers = defaultdict(list), Counter()
    with path.open() as f:
        for row in csv.DictReader(f):
            name, phase, target = row["model"], int(row["phase"]), float(row["target_km"])
            offers[(name, phase, target)] += 1
            if row["status"] != "scored":
                continue
            for field in ("actual_minutes", "predicted_minutes", "low_minutes", "high_minutes", "history_km",
                          "target_km", "distance_km", "unsupported_fraction"):
                row[field] = float(row[field])
            row["phase"], row["group"] = phase, int(row["group"])
            groups[name].append(row)
    return groups, offers


def rider_means(rows, field="ape"):
    values = defaultdict(list)
    for row in rows:
        actual, predicted = row["actual_minutes"], row["predicted_minutes"]
        value = (100 * abs(predicted / actual - 1) if field == "ape" else
                 100 * (row["low_minutes"] <= actual <= row["high_minutes"]) if field == "coverage" else
                 100 * (actual > row["high_minutes"]) if field == "upper" else
                 100 * (predicted / actual - 1))
        values[row["rider"]].append(value)
    return {rider: float(np.mean(items)) for rider, items in sorted(values.items())}


def mean_ci(values):
    values = np.array(list(values), dtype=float)
    if len(values) == 0:
        return [None, None]
    rng = np.random.default_rng(20260915)
    samples = values[rng.integers(0, len(values), size=(BOOTSTRAPS, len(values)))].mean(axis=1)
    return np.quantile(samples, [0.025, 0.975]).tolist()


def metrics(rows):
    if not rows:
        return dict(n=0, riders=0)
    actual = np.array([r["actual_minutes"] for r in rows])
    predicted = np.array([r["predicted_minutes"] for r in rows])
    low = np.array([r["low_minutes"] for r in rows])
    high = np.array([r["high_minutes"] for r in rows])
    error = 100 * abs(predicted / actual - 1)
    rw = list(rider_means(rows).values())
    coverage = list(rider_means(rows, "coverage").values())
    return dict(n=len(rows), riders=len(rw), rider_mape=float(np.mean(rw)), rider_mape_ci=mean_ci(rw),
                median_ape=float(np.median(error)), p90_ape=float(np.quantile(error, 0.9)),
                mean_absolute_minutes=float(np.mean(abs(predicted - actual))),
                rider_bias=float(np.mean(list(rider_means(rows, "bias").values()))),
                rider_coverage=float(np.mean(coverage)), rider_coverage_ci=mean_ci(coverage),
                pooled_coverage=float(np.mean((actual >= low) & (actual <= high)) * 100),
                rider_upper_exceedance=float(np.mean(list(rider_means(rows, "upper").values()))),
                median_width_minutes=float(np.median(high - low)),
                median_relative_width=float(np.median((high - low) / predicted) * 100))


def paired_improvement(left, right):
    left, right = rider_means(left), rider_means(right)
    users = sorted(set(left) & set(right))
    differences = [left[user] - right[user] for user in users]
    return dict(percentage_points=float(np.mean(differences)), ci=mean_ci(differences), riders=len(users))


def band(row):
    km = row["history_km"]
    if km == 0:
        return 0
    return int(np.searchsorted([50, 100, 300, 1000], km, side="right")) + 1


def table(headers, rows):
    return "| " + " | ".join(headers) + " |\n| " + " | ".join(["---"] * len(headers)) + " |\n" + "\n".join(
        "| " + " | ".join(str(x) for x in row) + " |" for row in rows) + "\n"


def save_plot(fig, output, name):
    fig.tight_layout()
    fig.savefig(output / f"{name}.svg", metadata={"Date": None})
    fig.savefig(output / f"{name}.png", dpi=150)
    plt.close(fig)


def plots(summary, output):
    plt.rcParams.update({"font.size": 11, "axes.spines.top": False, "axes.spines.right": False,
                         "axes.titleweight": "bold", "svg.fonttype": "none", "svg.hashsalt": "obc-ride-time-v1"})
    fig, ax = plt.subplots(figsize=(10, 5))
    positions = np.arange(len(MODELS))
    for phase, color in ((0, COLORS[0]), (1, COLORS[1])):
        items = [summary["phase"][name][str(phase)] for name in MODELS]
        means = np.array([m["rider_mape"] for m in items])
        ci = np.array([m["rider_mape_ci"] for m in items])
        ax.bar(positions + (phase - 0.5) * 0.36, means, 0.36, color=color,
               yerr=np.array([means - ci[:, 0], ci[:, 1] - means]), capsize=3,
               label="Ride start" if phase == 0 else "After 10 accepted minutes")
    ax.set_xticks(positions, [NAMES[n].replace(" ", "\n", 1) for n in MODELS])
    ax.set_ylabel("Rider-weighted mean absolute error (%)")
    ax.set_title("Personal pace and live adaptation improve this proxy")
    ax.legend(frameon=False)
    ax.set_ylim(bottom=0)
    save_plot(fig, output, "accuracy")

    fig, ax = plt.subplots(figsize=(10, 4.5))
    labels = ["0", "0–50", "50–100", "100–300", "300–1,000", "1,000+"]
    for name, color in zip(("fixed_grade", "personal_grade", "grade_live"), COLORS):
        ax.plot(labels, [m.get("rider_mape", np.nan) for m in summary["history"][name]],
                marker="o", color=color, label=NAMES[name])
    ax.set_xlabel("Accepted training distance before the ride (km)")
    ax.set_ylabel("Rider-weighted mean absolute error (%)")
    ax.set_title("Sparse-history results — both checkpoint phases")
    ax.legend(frameon=False)
    save_plot(fig, output, "learning")

    fig, ax = plt.subplots(figsize=(10, 4.5))
    items = summary["coverage_groups"]
    means = np.array([m.get("rider_coverage", np.nan) for m in items])
    ci = np.array([m.get("rider_coverage_ci", [np.nan, np.nan]) for m in items])
    ax.errorbar(np.arange(6), means, yerr=[means - ci[:, 0], ci[:, 1] - means], fmt="o",
                color=COLORS[1], capsize=4)
    ax.axhline(90, color=COLORS[2], linestyle="--", label="Nominal 90% target")
    ax.set_xticks(np.arange(6), [f"{phase}\n{duration}" for phase in ("Start", "In ride")
                              for duration in ("<10 min", "10–30 min", "30+ min")])
    ax.set_ylabel("Rider-weighted coverage (%)")
    ax.set_title("Observed interval coverage varies across forecast groups")
    ax.legend(frameon=False)
    save_plot(fig, output, "coverage")

    fig, ax = plt.subplots(figsize=(9, 4.5))
    items = summary["targets"]
    scored = np.array([r["scored"] for r in items])
    offered = np.array([r["offered"] for r in items])
    positions = np.arange(len(items))
    ax.bar(positions, scored, color=COLORS[1], label="Scored")
    ax.bar(positions, offered - scored, bottom=scored, color="#d8cbbb", label="Unscorable outcome")
    for i, (n, total) in enumerate(zip(scored, offered)):
        ax.text(i, total + 100, f"{100*n/total:.1f}% scored", ha="center", fontsize=10)
    ax.set_xticks(positions, [f"{r['target_km']:g} km" for r in items])
    ax.set_ylabel("Forecast targets (one model, both phases)")
    ax.set_title("Data-quality screens leave few long targets scorable")
    ax.set_ylim(0, float(offered.max()) * 1.15)
    ax.legend(frameon=False)
    save_plot(fig, output, "selection")

    fig, ax = plt.subplots(figsize=(9, 4.5))
    for factor, values in summary["diagnostics"]["synthetic"]["cold_start"].items():
        ax.plot(np.arange(len(values)) * 10, (np.array(values) - 1) * 100,
                label=f"Initial pace {factor}× correct pace")
    ax.axhline(0, color="#555", linewidth=1)
    ax.set_xlabel("Synthetic accepted training distance (km)")
    ax.set_ylabel("Flat-ground pace error (%)")
    ax.set_title("Conservative learning takes time to correct a poor prior")
    ax.legend(frameon=False)
    save_plot(fig, output, "cold-start")


def write_report(summary, output):
    phase = summary["phase"]
    main = phase["grade_live"]["1"]
    fixed = phase["fixed_grade"]["1"]
    simple = phase["scalar_live"]["1"]
    gain, small_gain = summary["improvement_vs_fixed"], summary["improvement_vs_scalar_live"]
    stats, audit = summary["test_stats"], summary["audit"]
    scored = summary["all"]["grade_live"]["n"]
    offered = sum(x["offered"] for x in summary["targets"])
    selection = summary["initial_curve"]["selection"]
    diag = summary["diagnostics"]
    mtb = summary["sports"]["mountain bike"]
    text = ["# Ride-time estimation: first FitRec evaluation\n",
            "**Status: experimental.** This report evaluates the reviewed Python prototype on held-out riders. "
            "It measures **movement-screened elapsed interval time**, not verified moving time. "
            "Short stops can remain inside a recorded interval.\n",
            "## Findings\n",
            f"- At in-ride checkpoints, **Gradient + live** has **{main['rider_mape']:.2f}% rider-weighted mean absolute percentage error**, "
            f"versus {fixed['rider_mape']:.2f}% for fixed gradient estimates. Median forecast error is "
            f"{main['median_ape']:.2f}%; the 90th percentile is {main['p90_ape']:.2f}%.\n"
            f"- The nominal 90% range covers **{main['rider_coverage']:.2f}%** of outcomes after giving each rider equal weight "
            f"(95% rider-bootstrap interval: {main['rider_coverage_ci'][0]:.2f}–{main['rider_coverage_ci'][1]:.2f}%).\n"
            f"- **Mountain-bike range coverage is only {mtb['rider_coverage']:.2f}%** across both checkpoint phases "
            f"({mtb['n']:,} targets from {mtb['riders']} riders). Aggregate coverage hides this important failure.\n"
            f"- The simpler **Scale + live** model reaches {simple['rider_mape']:.2f}% error. The extra personal gradient terms improve "
            f"the mean by only **{small_gain['percentage_points']:.2f} percentage points** "
            f"(95% interval {small_gain['ci'][0]:.2f}–{small_gain['ci'][1]:.2f}).\n"
            f"- Only **{scored:,} of {offered:,} available targets ({100*scored/offered:.1f}%)** can be scored under the primary "
            "recording-quality rule. This selected cohort does not establish all-day ETA, pushing, or two-hour water-gap accuracy.\n",
            "**Assessment:** personal pace learning and live adjustment merit further work. The extra gradient learner has a small "
            "incremental benefit in this experiment. The prediction ranges are not ready for device use: mountain-bike coverage "
            "falls well below the stated level. Dense original rides and reliable route attributes are needed to establish "
            "performance on the intended bikepacking use cases.\n",
            "## 1. Review and implementation\n",
            "An independent adversarial reviewer and the primary agent agreed the algorithm was ready for an experimental prototype. "
            "The review verified the centered summaries, quadratic ride-offset elimination and bounded coordinate update. It found "
            "three issues that were corrected before the final test:\n\n"
            "1. Per-ride coefficient movement now scales with accepted evidence: `Delta = s_ride * delta`. Tiny rides cannot each "
            "unlock a full step toward old evidence.\n"
            "2. Overlapping recordings are excluded. A complete earlier-starting ride cannot teach the model about a later record's future.\n"
            "3. Nonfinite objective values explicitly reject a numerical update.\n\n"
            "The reviewer confirmed those implementation corrections. No test-set outcome was used to choose settings. "
            "The current multiplier applies to the entire remaining route, without forecast decay.\n",
            "## 2. Dataset and experiment boundaries\n",
            "[FitRec / Endomondo](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html) was selected because it provides repeated "
            "cycling histories with rider identifiers, positions, timestamps and altitude. SimRa does not supply persistent rider "
            "identifiers; GeoLife has a more limited cycling-labelled subset. FitRec therefore best fits this first longitudinal experiment.\n",
            table(["Stage", "Riders", "Records"], [
                ["Raw cycling candidates", audit["cycling_users"], audit["counts"]["cycling_records"]],
                ["Structurally usable cycling records", sum(audit["users"].values()), audit["counts"]["stored_records"]],
                ["Development: initial-curve fitting", audit["users"]["development"],
                 selection["candidate_records"] - selection.get("overlapping_records_excluded", 0) - selection.get("nonpositive_duration_records_excluded", 0)],
                ["Separate interval calibration", summary["calibration"]["stats"]["users"], summary["calibration"]["stats"]["rides"]],
                ["Held-out replay", stats["users"], stats["rides"]],
            ]),
            f"The raw file contains {audit['counts']['all_records']:,} workouts across all sports. The cycling extraction rejects "
            f"{audit['exclusions'].get('array_lengths', 0)} records with unusable array lengths. "
            f"{audit['point_counts']['500']:,} usable cycling records contain exactly 500 points. Across stored cycling intervals, "
            f"rounded timestamp spacing has median {audit['rounded_spacing_quantiles_seconds']['0.5']} seconds and "
            f"95th percentile {audit['rounded_spacing_quantiles_seconds']['0.95']} seconds.\n\n"
            "Each rider is assigned to development, calibration or test by a fixed SHA-256 rule. Only the first 60 candidate rides "
            "per rider are considered, in chronological order; overlap exclusions do not refill that quota. The test cohort excludes "
            f"{stats['overlapping_records_excluded']} overlapping records. These are disjoint rider sets, not random GPS-point splits.\n\n"
            "The real-data model uses **one pooled cycling context plus gradient**. Some activities are labelled mountain bike, "
            "but generic bike labels do not establish road, gravel or touring categories. There are no surface labels or map matching "
            "in this replay. Bike/surface transfer is tested only on synthetic data.\n",
            "### Timing and route assumptions\n",
            "A scored target requires every intervening interval to have positive spacing at most 30 seconds, GPS displacement at least "
            "3 metres, interval-average GPS speed from 1 to 80 km/h, and a finite usable elevation profile. These are dataset screens, "
            "not a validated firmware motion classifier. They exclude slow pushing and some genuine difficult terrain. "
            "A short stop inside an accepted interval remains possible.\n\n"
            "Distance is the sum of GPS chords. Gradient is a trailing spatial secant over approximately 200 metres of recorded altitude. "
            "Features clamp to the outer ±20% anchors; profile slopes above ±50% fail the data screen. The recorded geometry and altitude "
            "serve as a known-route proxy. Real planned-route and map-elevation errors are absent from this experiment.\n\n"
            "Forecasts are issued at ride start and the first boundary after 10 accepted minutes. Targets are the next recorded route "
            "point at least 1, 3, 10 or 30 km ahead, chosen before outcomes are inspected. Targets beyond the recorded route are not offered. "
            "An intervening invalid interval makes the outcome unscorable; separated windows are never joined. Baseline evidence is "
            "committed only at the original ride boundary.\n",
            "![Scored and unscorable forecast targets by route distance](selection.svg)\n",
            table(["Target", "Offered", "Scored", "Scored fraction"], [
                [f"{r['target_km']:g} km", f"{r['offered']:,}", f"{r['scored']:,}", f"{100*r['scored']/r['offered']:.1f}%"]
                for r in summary["targets"]]),
            f"Number of scored targets with at least two hours of observed elapsed time: **{summary['long_actual_count']}**. "
            "This count describes selected completed targets; it does not establish coverage of two-hour gaps in general.\n",
            "## 3. Accuracy on held-out riders\n",
            "The main metric is **rider-weighted mean absolute percentage error**: first average absolute relative errors within "
            "each rider, then average riders equally. This prevents prolific recorders from dominating. Median and 90th-percentile "
            "errors, and mean absolute minutes, pool individual forecasts. A 10% error on a 30-minute target corresponds to 3 minutes.\n\n"
            "All models see the same issued targets and outcome screen. Fixed pace and the fixed gradient curve come from development "
            "riders. Personal scale learns one correction to that curve. Personal gradient also learns gradient-anchor corrections. "
            "The two live variants apply their current multiplier to the whole forecast.\n",
            "![Accuracy comparison at ride start and after ten accepted minutes](accuracy.svg)\n",
            "### After 10 accepted minutes\n",
            table(["Model", "Rider-weighted error", "Median error", "90th-percentile error", "Mean absolute minutes"], [
                [NAMES[n], f"{phase[n]['1']['rider_mape']:.2f}%", f"{phase[n]['1']['median_ape']:.2f}%",
                 f"{phase[n]['1']['p90_ape']:.2f}%", f"{phase[n]['1']['mean_absolute_minutes']:.2f}"] for n in MODELS]),
            f"This phase contains {main['n']:,} scored targets from {main['riders']} riders. The paired improvement over fixed "
            f"gradient estimates is {gain['percentage_points']:.2f} percentage points, with a 95% rider-bootstrap interval of "
            f"{gain['ci'][0]:.2f}–{gain['ci'][1]:.2f}. Bootstrap resampling keeps each rider's records together "
            f"({BOOTSTRAPS:,} resamples, fixed seed). These intervals describe uncertainty across sampled riders, not a per-route guarantee.\n",
            f"The rider-weighted signed relative error is **{main['rider_bias']:+.2f}%**. Positive values mean the model "
            "overestimates duration on average; the point estimates retain a directional bias.\n",
            "### At ride start\n",
            table(["Model", "Rider-weighted error", "Median error", "90th-percentile error"], [
                [NAMES[n], f"{phase[n]['0']['rider_mape']:.2f}%", f"{phase[n]['0']['median_ape']:.2f}%",
                 f"{phase[n]['0']['p90_ape']:.2f}%"] for n in MODELS[:4]]),
            "Live multipliers start at one, so their ride-start estimates equal the corresponding personal baseline. "
            "Start and in-ride rows have different eligible targets; compare models within a phase rather than treating "
            "the phase difference as a controlled experiment.\n",
            "## 4. Sparse personal histories\n",
            "![Error by accepted training distance before the ride](learning.svg)\n",
            table(["Prior accepted km", "Riders", "Targets", "Fixed gradient", "Personal gradient", "Gradient + live"], [
                [label, summary["history"]["grade_live"][i]["riders"], summary["history"]["grade_live"][i]["n"],
                 *[f"{summary['history'][n][i].get('rider_mape', float('nan')):.2f}%"
                   for n in ("fixed_grade", "personal_grade", "grade_live")]]
                for i, label in enumerate(("0", ">0–50", "50–100", "100–300", "300–1,000", "1,000+"))]),
            "Both checkpoint phases are included. Distance counts accepted baseline-learning observations, not annual riding distance. "
            "Rider and route composition changes across bins. The chart is a diagnostic comparison, not proof of a causal learning "
            "curve for every rider.\n",
            "## 5. Prediction ranges\n",
            "Reference error distributions come from separate calibration riders. Each personal buffer retains 32 errors per group; "
            "the reference distribution has weight 32. The six groups combine ride phase with predicted duration below 10, 10–30, "
            "or at least 30 minutes. One calibration slot per group is reserved prospectively, in ascending target-distance order. "
            "A censored slot is not replaced with a later successful target.\n\n"
            "This is an empirical mixture, not a conformal coverage guarantee. Full valid errors remain in calibration; the "
            "baseline learner's clipping does not erase tail errors.\n",
            "![Observed interval coverage by phase and predicted duration](coverage.svg)\n",
            table(["Forecast group", "Targets", "Riders", "Coverage", "Above upper bound", "Median range width"], [
                [f"{'Start' if i < 3 else 'In ride'} / {('<10 min','10–30 min','30+ min')[i%3]}", m["n"], m["riders"],
                 f"{m.get('rider_coverage', float('nan')):.2f}%", f"{m.get('rider_upper_exceedance', float('nan')):.2f}%",
                 f"{m.get('median_width_minutes', float('nan')):.2f} min"] for i, m in enumerate(summary["coverage_groups"])]),
            "Coverage and upper exceedance give each rider equal weight. Width pools forecasts. Good aggregate coverage does not "
            "establish coverage for a particular unfamiliar surface, rider or water source.\n",
            "### Activity-label subgroups\n",
            table(["Recorded sport", "Riders", "Targets", "Rider-weighted error", "Coverage"], [
                [sport, m["riders"], m["n"], f"{m['rider_mape']:.2f}%", f"{m['rider_coverage']:.2f}%"]
                for sport, m in summary["sports"].items()]),
            "These rows use Gradient + live at both phases. Activity labels are subgroup descriptions, not reliable bike/surface "
            "inputs to this reduced model. Mountain-bike coverage is a failure of the current range method on this selected subgroup. "
            "The pooled model and missing terrain attributes are possible contributors, but this experiment does not identify the cause. "
            "These held-out results must not be used to tune a replacement and then claim a new independent test on the same riders.\n",
            "## 6. Stricter sampling sensitivity\n",
            table(["Maximum spacing", "Scored targets, both phases", "In-ride error", "In-ride coverage"], [
                ["30 seconds", scored, f"{main['rider_mape']:.2f}%", f"{main['rider_coverage']:.2f}%"],
                ["15 seconds", summary["sensitivity"]["all"]["n"],
                 f"{summary['sensitivity']['in_ride']['rider_mape']:.2f}%", f"{summary['sensitivity']['in_ride']['rider_coverage']:.2f}%"]]),
            "The 15-second screen was specified before the final test. It uses the same configuration and 30-second calibration "
            "reference. It changes the selected cohort, learning evidence, and some in-ride checkpoint positions. It is a robustness "
            "check, not a paired comparison of identical routes or a separately calibrated 15-second estimator.\n",
            "## 7. Synthetic robustness and numerical checks\n",
            "Synthetic cases test expected behaviour under controlled inputs. They do not establish real-world accuracy.\n",
            "![Recovery from half and double the appropriate initial pace](cold-start.svg)\n",
            table(["Case", "Observed result"], [
                ["One 50% slower ride after 20 stable rides", f"Flat baseline changes {100*diag['synthetic']['single_slow_ride_baseline_change']:.2f}%"],
                ["Unobserved MTB + rough surface + climb combination", f"{100*(diag['synthetic']['unseen_bike_surface_climb_ratio']-1):+.2f}% pace error after 100 synthetic training rides"],
                ["Float32 versus float64, 120 development rides", f"Maximum grid pace difference {100*diag['numerical']['max_float32_vs_float64_pace_difference']:.6f}%"],
                ["Eight sweeps versus independent bounded least-squares solver", f"Maximum grid pace difference {100*diag['numerical']['max_fixed_sweeps_vs_reference_pace_difference']:.6f}%"],
                ["Reference solver failures", diag["numerical"]["reference_solver_failures"]],
                ["Rejected baseline updates in primary held-out replay", stats["rejected_baseline_updates"]],
            ]),
            "The cold-prior curves expose a real tradeoff: clipping and conservative updates delay correction of a poor default. "
            "The synthetic transfer case also includes regularization bias. The converged reference uses an independent bounded "
            "least-squares solve of the same quadratic; it does not revisit historical clipping.\n",
            "## 8. Compute and memory\n",
            table(["Configuration", "Learner arrays", "Interval arrays + live scalars", "Subtotal"], [
                [f"{p} coefficients", f"{r['learner_array_bytes']:,} B", f"{r['interval_array_bytes']+r['live_scalar_bytes']:,} B",
                 f"{r['subtotal_array_bytes']:,} B"] for p, r in diag["resources"].items()]),
            "Nine coefficients are used in the FitRec gradient-only replay. Sixteen represent the illustrative four-bike, "
            "five-surface, nine-anchor model. These are actual NumPy array payload sizes, plus the two live scalars. "
            "They exclude Python objects, pending forecasts, input preprocessing, route storage and temporary arrays. "
            "Reference distributions are fixed data suitable for flash.\n\n"
            f"On this host, the median nine-coefficient finish step was {diag['numerical']['finish_ms_quantiles']['0.5']:.3f} ms "
            f"and the 99th percentile was {diag['numerical']['finish_ms_quantiles']['0.99']:.3f} ms. "
            f"The primary six-model replay took {stats['runtime_seconds']:.1f} seconds for {stats['rides']:,} rides. "
            "Some measurements ran alongside another replay. These are host measurements, not nRF54 estimates.\n\n"
            f"Python tracing of learner construction and updates peaked at {diag['resources']['16']['python_traced_during_ride_peak_bytes']:,} B "
            f"for 16 coefficients, and {diag['resources']['16']['python_traced_including_finish_peak_bytes']:,} B including finalization. "
            "This trace covers the learner, not the complete Python process or the interval component. The replay caches input "
            "route arrays on the host. Only the learner state is intended to remain fixed-size on the device.\n",
            "## 9. What remains before device integration\n",
            "- Validate actual moving time, stops and pushing on dense original recordings.\n"
            "- Add reliable surface association and bike labels before judging full-model transfer.\n"
            "- Validate long forecasts on data that can actually score them.\n"
            "- Compare the small gain from personal gradient corrections with their implementation cost on new development data.\n"
            "- Address the mountain-bike coverage failure and establish an uncertainty policy for unsupported terrain and combinations.\n"
            "- Benchmark the Rust kernel and complete working memory on the nRF54LM20.\n",
            "## 10. Reproduction and provenance\n",
            "The [prototype README](src:host/ride-time-prototype/README.md) gives the complete commands. "
            "The aggregate result and frozen configuration are stored with the prototype. Downloaded records and per-forecast "
            "CSV files remain in local caches; this report includes no GPS tracks or rider identifiers.\n\n"
            "Source citation: Jianmo Ni, Larry Muhlstein and Julian McAuley, *Modeling heart rate and activity data for personalized "
            "fitness recommendation*, WWW 2019. [Dataset and published terms](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html).\n\n"
            f"Raw archive: {audit['source_bytes']:,} bytes. SHA-256:\n\n```text\n{audit['source_sha256']}\n```\n",
            "Configuration, reference-distribution and estimator-source hashes were frozen before held-out evaluation. "
            "The report checks that they still match. The result remains an initial experiment, not a production accuracy claim.\n"]
    markdown = "\n".join(text)
    (output / "report.md").write_text(markdown)
    sys.path.insert(0, str(ROOT / "docs"))
    from build_docs import render_blocks
    content, _ = render_blocks(markdown)
    for name in ("accuracy", "learning", "coverage", "selection", "cold-start"):
        encoded = base64.b64encode((output / f"{name}.svg").read_bytes()).decode()
        content = content.replace(f'src="{name}.svg"', f'src="data:image/svg+xml;base64,{encoded}"')
    css = """body{max-width:1100px;margin:0 auto;padding:40px 24px;background:#faf8f1;color:#21382e;
font:17px/1.65 system-ui,sans-serif}h1{font-size:2.3rem;line-height:1.2}h2{margin-top:2.6em;border-top:1px solid #d9ddcf;padding-top:1em}
h3{margin-top:1.6em}a{color:#315e4b}img{width:100%;height:auto;background:white;border-radius:8px;margin:1em 0}
table{width:100%;border-collapse:collapse;font-size:14px;display:block;overflow-x:auto}th,td{text-align:left;padding:10px 13px;border-bottom:1px solid #d9ddcf}
th{background:#e6ecdf}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#e6ecdf;padding:16px}code{font-size:.87em}
@media print{body{background:white;padding:0;font-size:11pt}h2{break-after:avoid}img,table{break-inside:avoid}}"""
    (output / "report.html").write_text(f'<!doctype html><html lang="en"><head><meta charset="utf-8">'
                                       f'<meta name="viewport" content="width=device-width,initial-scale=1">'
                                       f'<title>Ride-time estimation: first FitRec evaluation</title><style>{css}</style>'
                                       f'</head><body>{content}</body></html>')
    return markdown


def freeze(output):
    path = output / "frozen-manifest.json"
    if path.exists():
        raise SystemExit("A frozen manifest already exists. Use a fresh experiment output directory.")
    files = [Path(__file__).parent / name for name in ("model.py", "data.py", "replay.py")]
    files += [output / name for name in ("config.json", "reference.json")]
    manifest = dict(purpose="Frozen before held-out evaluation", python=sys.version, platform=platform.platform(),
                    libraries=dict(numpy=np.__version__, scipy=scipy.__version__, matplotlib=matplotlib.__version__),
                    sha256={str(p.relative_to(ROOT) if p.is_absolute() and p.is_relative_to(ROOT) else p):
                            hashlib.sha256(p.read_bytes()).hexdigest() for p in files})
    path.write_text(json.dumps(manifest, indent=2) + "\n")


def build(output, publish):
    manifest = json.loads((output / "frozen-manifest.json").read_text())
    for name, digest in manifest["sha256"].items():
        if hashlib.sha256(Path(name).read_bytes()).hexdigest() != digest:
            raise SystemExit(f"Frozen input changed: {name}")
    records, offers = load(output / "test-30s.csv")
    stricter, _ = load(output / "test-15s.csv")
    chosen = records["grade_live"]
    phase = {name: {str(p): metrics([r for r in rows if r["phase"] == p]) for p in (0, 1)}
             for name, rows in records.items()}
    summary = dict(phase=phase, all={name: metrics(rows) for name, rows in records.items()},
                   history={name: [metrics([r for r in rows if band(r) == i]) for i in range(6)]
                            for name, rows in records.items()},
                   coverage_groups=[metrics([r for r in chosen if r["group"] == g]) for g in range(6)],
                   sports={s: metrics([r for r in chosen if r["sport"] == s]) for s in sorted({r["sport"] for r in chosen})},
                   targets=[dict(target_km=t, scored=sum(r["target_km"] == t for r in chosen),
                                 offered=sum(offers[("grade_live", p, t)] for p in (0, 1))) for t in (1, 3, 10, 30)],
                   sensitivity=dict(all=metrics(stricter["grade_live"]),
                                    in_ride=metrics([r for r in stricter["grade_live"] if r["phase"] == 1])),
                   long_actual_count=sum(r["actual_minutes"] >= 120 for r in chosen),
                   improvement_vs_fixed=paired_improvement([r for r in records["fixed_grade"] if r["phase"] == 1],
                                                          [r for r in chosen if r["phase"] == 1]),
                   improvement_vs_scalar_live=paired_improvement([r for r in records["scalar_live"] if r["phase"] == 1],
                                                                [r for r in chosen if r["phase"] == 1]),
                   audit=json.loads((CACHE / "fitrec.audit.json").read_text()),
                   diagnostics=json.loads((output / "diagnostics.json").read_text()),
                   test_stats=json.loads((output / "test-30s-summary.json").read_text()),
                   calibration=json.loads((output / "calibration-summary.json").read_text()),
                   initial_curve=json.loads((output / "initial-curve.json").read_text()),
                   config=json.loads((output / "config.json").read_text()), frozen_manifest=manifest)
    summary["audit"]["point_counts"] = {"500": summary["audit"]["point_counts"]["500"]}
    (output / "aggregate-results.json").write_text(json.dumps(summary, indent=2) + "\n")
    plots(summary, output)
    markdown = write_report(summary, output)
    if publish:
        assets = ROOT / "docs/assets/research/ride-time"
        assets.mkdir(parents=True, exist_ok=True)
        for name in ("accuracy", "learning", "coverage", "selection", "cold-start"):
            shutil.copyfile(output / f"{name}.svg", assets / f"{name}.svg")
            markdown = markdown.replace(f"]({name}.svg)", f"](../../assets/research/ride-time/{name}.svg)")
        front = "---\ntitle: Ride time estimation — first evaluation\ndescription: Held-out FitRec results and limits of the experimental pace learner.\ncopy: ai\n---\n\n"
        (ROOT / "docs/content/software/ride-time-evaluation.md").write_text(front + markdown)
        results = Path(__file__).parent / "results"
        results.mkdir(exist_ok=True)
        shutil.copyfile(output / "aggregate-results.json", results / "fitrec-v1.json")
    print(json.dumps({"in_ride": phase["grade_live"]["1"], "vs_fixed": summary["improvement_vs_fixed"],
                      "vs_scalar_live": summary["improvement_vs_scalar_live"], "targets": summary["targets"]}, indent=2))
    print(f"Report: {output / 'report.html'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(".artifacts/ride-time"))
    parser.add_argument("--freeze", action="store_true")
    parser.add_argument("--publish-docs", action="store_true", help="Write aggregate report and charts to local docs source")
    args = parser.parse_args()
    if args.freeze:
        freeze(args.output)
    else:
        build(args.output, args.publish_docs)
