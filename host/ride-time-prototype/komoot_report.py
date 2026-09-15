"""Private, reproducible report for the frozen Komoot recorded-motion replay."""

import argparse
import base64
import csv
import html
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

from komoot_data import OUTPUT, SOURCE
from komoot_replay import BUDGETS, verify
from long_data import digest

CHECKPOINTS = (0, 10, 30, 60)
LABELS = {"baseline": "Scalar + live", "gradient": "With uphill correction"}
COLORS = {"baseline": "#365d3c", "gradient": "#d57851"}


def metrics(rows):
    if not rows:
        return dict(n=0)
    actual = np.array([r["actual_minutes"] for r in rows])
    predicted = np.array([r["predicted_minutes"] for r in rows])
    error = predicted-actual
    ape = 100*np.abs(error)/actual
    result = dict(n=len(rows), mape=float(ape.mean()), median_ape=float(np.median(ape)),
                  p90_ape=float(np.quantile(ape, .9)), mae_minutes=float(np.abs(error).mean()),
                  bias_minutes=float(error.mean()))
    if all(r["low_minutes"] is not None for r in rows):
        low, high = np.array([[r["low_minutes"], r["high_minutes"]] for r in rows]).T
        result.update(coverage=100*float(((low <= actual) & (actual <= high)).mean()),
                      median_width_minutes=float(np.median(high-low)))
    return result


def paired_difference(a, b):
    first, second = {r["ride"]: r for r in a}, {r["ride"]: r for r in b}
    if len(first) != len(a) or len(second) != len(b) or first.keys() != second.keys():
        raise ValueError("Comparison is not one-to-one paired by ride")
    differences = []
    for id, x in first.items():
        y = second[id]
        if (x["actual_minutes"], x["at_minutes"]) != (y["actual_minutes"], y["at_minutes"]):
            raise ValueError("Paired models have different targets")
        differences.append(100*(abs(y["predicted_minutes"]-y["actual_minutes"])
                                 -abs(x["predicted_minutes"]-x["actual_minutes"]))/x["actual_minutes"])
    d = np.array(differences)
    return dict(n=len(d), mean_delta_pp=float(d.mean()) if len(d) else None,
                median_delta_pp=float(np.median(d)) if len(d) else None,
                improved=int((d < -1e-9).sum()), worsened=int((d > 1e-9).sum()))


def load(output):
    with (output/"predictions.csv").open() as f:
        rows = list(csv.DictReader(f))
    strings = {"scenario", "mode", "ride", "bike"}
    for r in rows:
        for k, v in list(r.items()):
            if k not in strings:
                r[k] = float(v) if v else None
        if not (r["actual_minutes"] > 0 and r["predicted_minutes"] > 0):
            raise ValueError("Invalid prediction/outcome")
    keys = [(r["scenario"], r["mode"], r["ride"], r["history_target_km"], r["checkpoint"]) for r in rows]
    if len(keys) != len(set(keys)):
        raise ValueError("Duplicate forecast keys")
    return rows


def summarize(rows):
    continuous = [r for r in rows if r["scenario"] == "continuous"]
    summary = dict(continuous={}, paired={}, comparisons={})
    groups = {"all": lambda r: True, "long": lambda r: r["ride_minutes"] >= 120,
              "2–4 h": lambda r: 120 <= r["ride_minutes"] < 240,
              "4–6 h": lambda r: 240 <= r["ride_minutes"] < 360,
              "6+ h": lambda r: r["ride_minutes"] >= 360,
              "MTB": lambda r: r["bike"] == "mtb", "other": lambda r: r["bike"] == "other"}
    for group, predicate in groups.items():
        summary["continuous"][group] = {}
        summary["comparisons"][group] = {}
        for checkpoint in CHECKPOINTS:
            selected = {mode: [r for r in continuous if r["mode"] == mode
                              and r["checkpoint"] == checkpoint and predicate(r)] for mode in LABELS}
            summary["continuous"][group][str(checkpoint)] = {m: metrics(rs) for m, rs in selected.items()}
            summary["comparisons"][group][str(checkpoint)] = paired_difference(*selected.values())
    for budget in BUDGETS:
        summary["paired"][str(budget)] = {}
        for checkpoint in CHECKPOINTS:
            selected = {mode: [r for r in rows if r["scenario"] == "paired" and r["mode"] == mode
                              and r["history_target_km"] == budget and r["checkpoint"] == checkpoint]
                        for mode in LABELS}
            paired_difference(*selected.values())
            summary["paired"][str(budget)][str(checkpoint)] = {m: metrics(rs) for m, rs in selected.items()}
    return summary


class Report:
    def __init__(self):
        self.md, self.body = [], []

    def heading(self, text, level=2):
        self.md.append("#"*level+" "+text)
        self.body.append(f"<h{level}>{html.escape(text)}</h{level}>")

    def paragraph(self, text):
        self.md.append(text)
        self.body.append("<p>"+html.escape(text)+"</p>")

    def table(self, headings, rows):
        self.md.append("\n".join(["| "+" | ".join(headings)+" |", "| "+" | ".join(["---"]*len(headings))+" |"]
                                  +["| "+" | ".join(map(str, r))+" |" for r in rows]))
        self.body.append("<div class='scroll'><table><thead><tr>"+"".join("<th>"+html.escape(h)+"</th>" for h in headings)
                         +"</tr></thead><tbody>"+"".join("<tr>"+"".join("<td>"+html.escape(str(c))+"</td>" for c in r)+"</tr>" for r in rows)+"</tbody></table></div>")

    def figure(self, path, caption):
        self.md.append(f"![{caption}]({path.name})")
        encoded = base64.b64encode(path.read_bytes()).decode()
        self.body.append(f"<figure><img alt='{html.escape(caption, quote=True)}' src='data:image/svg+xml;base64,{encoded}'><figcaption>{html.escape(caption)}</figcaption></figure>")

    def save(self, output):
        (output/"report.md").write_text("\n\n".join(self.md)+"\n")
        style = "body{font:17px/1.55 system-ui;max-width:1080px;margin:40px auto;padding:0 24px;color:#26332c;background:#faf8f2}h1,h2{line-height:1.2}h2{margin-top:2em}table{border-collapse:collapse;width:100%;font-size:15px}td,th{padding:9px;text-align:left;border-bottom:1px solid #ddd}th{background:#e7eddf}.scroll{overflow:auto}img{width:100%}figure{margin:30px 0}figcaption{font-size:14px;color:#555}"
        (output/"report.html").write_text("<!doctype html><html lang='en'><meta charset='utf-8'><meta name='viewport' content='width=device-width'><title>Komoot ride-time replay</title><style>"+style+"</style><body>"+"\n".join(self.body)+"</body></html>")


def report(source, output):
    verify(source, output)
    run = json.loads((output/"run.json").read_text())
    if any(digest(output/name) != expected for name, expected in run["hashes"].items()):
        raise ValueError("Replay output changed")
    rows = load(output)
    summary = summarize(rows)
    plan = json.loads((output/"protocol.json").read_text())
    audit = json.loads((output/"audit.json").read_text())
    rides = json.loads((output/"rides.json").read_text())
    history = json.loads((output/"history.json").read_text())
    long = summary["continuous"]["long"]
    if long["10"]["baseline"]["n"] != len(plan["long_proxy_targets"]):
        raise ValueError("Primary target count differs from frozen cohort")
    r = Report()
    r.heading("Ride-time estimation on your Komoot history", 1)
    r.paragraph("Private research report · one rider · fixed chronological evaluation · recorded moving sections only")
    r.heading("What the test found")
    base, candidate = long["10"]["baseline"], long["10"]["gradient"]
    change = summary["comparisons"]["long"]["10"]
    r.paragraph(f"On {base['n']} rides of at least two observed moving hours, the baseline's mean absolute percentage error after ten observed minutes was {base['mape']:.1f}%. The uphill candidate scored {candidate['mape']:.1f}%. Its paired change was {change['mean_delta_pp']:+.2f} percentage points; negative means improvement.")
    r.paragraph(f"The candidate improved {change['improved']} rides and worsened {change['worsened']}. Baseline nominal-90% ranges covered {base['coverage']:.1f}% of these outcomes. Their median width was {base['median_width_minutes']:.0f} minutes.")
    r.paragraph("These are predictions for the remaining accepted sections, with unknown intervals removed from both the route proxy and outcome. They are not exact full-ride ETA errors. One rider's repeated rides do not establish population accuracy or guaranteed range coverage.")
    r.heading("What to do next")
    improvements = sum(summary['comparisons'][g]['10']['mean_delta_pp'] < 0 for g in ('2–4 h', '4–6 h', '6+ h') if summary['comparisons'][g]['10']['n'])
    r.paragraph(f"The uphill coefficient improves {improvements} of the three long-duration groups on average. Read that alongside the paired wins/losses and small MTB sample below. Keep the selected baseline for now and retain this candidate for an independent test; this single-rider result does not establish a general improvement.")
    if base['coverage'] < 90:
        r.paragraph(f"The first priority is long-ride range calibration: the nominal 90% target was not reached ({base['coverage']:.1f}% observed coverage). Use these results as diagnostic evidence, then test any changed defaults or range method on new reserved rides. Do not tune here and call a repeat run an independent validation.")
    r.heading("Accuracy as a long ride progresses")
    table = []
    for c in CHECKPOINTS:
        for mode in LABELS:
            m = long[str(c)][mode]
            table.append([c, LABELS[mode], m['n'], f"{m['mape']:.1f}%", f"{m['median_ape']:.1f}%", f"{m['p90_ape']:.1f}%", f"{m['mae_minutes']:.1f}", f"{m['bias_minutes']:+.1f}"])
    r.table(["Observed min", "Model", "Rides", "Mean APE", "Median APE", "90th percentile APE", "Mean abs. error, min", "Mean signed error, min"], table)
    r.paragraph("Each cell gives each ride one vote. Percentage error uses the actual remaining observed moving time. Negative signed error means an optimistic estimate. Checkpoints use the first accepted block boundary at or after the requested time.")
    r.heading("After how many recorded kilometres?")
    table = []
    for budget in BUDGETS:
        actual = [x['km'] for x in history if x['budget'] == budget]
        m = summary['paired'][str(budget)]
        table.append([budget, f"{np.median(actual):.0f}" if actual else '—', m['10']['baseline']['n'],
            *[f"{m[str(c)][mode]['mape']:.1f}%" if m[str(c)][mode]['n'] else '—' for c, mode in [(0,'baseline'),(10,'baseline'),(0,'gradient'),(10,'gradient')]]])
    r.table(["Requested km", "Median actual km", "Same target rides", "Baseline start", "Baseline 10 min", "Uphill start", "Uphill 10 min"], table)
    r.paragraph("Each budget starts a fresh device before the latest earlier whole rides needed to reach that distance. The target rides stay the same across budgets and models. Whole-ride inclusion can exceed the requested distance. Zero history still gets the first ten minutes of live adaptation. These are simulated ownership histories for one rider.")
    fig, axes = plt.subplots(1, 2, figsize=(11, 4), constrained_layout=True)
    for ax, checkpoint in zip(axes, (0, 10)):
        for mode in LABELS:
            ax.plot(range(len(BUDGETS)), [summary['paired'][str(b)][str(checkpoint)][mode].get('mape', np.nan) for b in BUDGETS], marker='o', color=COLORS[mode], label=LABELS[mode])
        ax.set(xticks=range(len(BUDGETS)), xticklabels=BUDGETS, xlabel='Requested prior recorded km', ylabel='Mean absolute percentage error (%)', title=f'{checkpoint} observed minutes into ride')
        ax.grid(alpha=.2); ax.legend(fontsize=9)
    path = output/'history.svg'; fig.savefig(path); fig.savefig(output/'history.png', dpi=140); plt.close(fig)
    r.figure(path, "Paired history experiment on identical long rides. Lower error is better; no population confidence bands are claimed.")
    r.heading("Duration and bike categories")
    table = []
    for group in ('all', '2–4 h', '4–6 h', '6+ h', 'MTB', 'other'):
        m = summary['continuous'][group]['10']; d = summary['comparisons'][group]['10']
        table.append([group, m['baseline']['n'], *[f"{m[mode]['mape']:.1f}%" if m[mode]['n'] else '—' for mode in LABELS],
                      f"{d['mean_delta_pp']:+.2f}" if d['n'] else '—', f"{d['improved']}/{d['worsened']}"])
    r.table(['Group at 10 min', 'Rides', 'Baseline mean APE', 'Uphill mean APE', 'Paired change, pp', 'Improved/worse'], table)
    r.paragraph("Duration groups use observed total movement. Bike groups include all eligible durations with a ten-minute forecast. Source touring/road labels share the other-bike prior. Few MTB rides cannot establish MTB accuracy.")
    r.heading("How honest were the baseline ranges?")
    r.table(["Observed min, long rides", "Rides", "Nominal-90% coverage", "Median width, min"],
        [[c, long[str(c)]["baseline"]["n"], f"{long[str(c)]['baseline']['coverage']:.1f}%",
          f"{long[str(c)]['baseline']['median_width_minutes']:.1f}"] for c in CHECKPOINTS])
    table = []
    for group in ('long', '2–4 h', '4–6 h', '6+ h', 'MTB', 'other'):
        m = summary['continuous'][group]['10']['baseline']
        table.append([group, m['n'], f"{m['coverage']:.1f}%" if m['n'] else '—', f"{m['median_width_minutes']:.1f}" if m['n'] else '—'])
    r.table(['Group', 'Rides', 'Nominal-90% coverage', 'Median range width, min'], table)
    r.paragraph("References come unchanged from FitRec. Personal range errors enter only after an eligible ride ends, at most once per phase/duration group per ride. These intervals were originally calibrated for a different dataset and target definition. The uphill candidate has no validated ranges.")
    r.heading("Where the data limits the answer")
    r.paragraph(f"The source has {audit['source_rides']} recordings. Preparation retained {audit['retained']} after overlap screening. Of {len(plan['cohort']['evaluation'])} evaluation-period recordings, {len(plan['proxy_targets'])} pass the conditional proxy screen. None of the long rides passes the strict zero-unknown screen.")
    evaluation = [x for x in rides if x['id'] in set(plan['cohort']['evaluation'])]
    table = []
    for name, lower, upper in [('<2 h',0,120), ('2–4 h',120,240), ('4–6 h',240,360), ('6+ h',360,float('inf'))]:
        cohort = [x for x in evaluation if lower <= x['source_moving_minutes'] < upper]
        accepted = [x for x in cohort if x['proxy_eligible']]
        table.append([name,len(cohort),len(accepted),
            f"{np.median([100*x['distance_coverage'] for x in cohort]):.2f}%" if cohort else '—',
            f"{np.median([x['unknown_minutes'] for x in cohort]):.1f}" if cohort else '—',
            f"{np.median([100*(x['moving_minutes']/x['source_moving_minutes']-1) for x in accepted]):+.2f}%" if accepted else '—'])
    r.table(['Source moving duration', 'Retained rides', 'Proxy eligible', 'Median distance coverage (all)', 'Median unknown min (all)', 'Median proxy/source time difference (eligible)'], table)
    r.paragraph("This table groups by Komoot source duration, so its counts can differ from observed-duration tables. Unknown time can include breaks or missing movement. The 99% criterion concerns recorded straight-line GPS distances, not proof of the true distance travelled. Observed time must agree with the source moving total within max(120 seconds, 5%); no missing time is redistributed to force agreement.")
    counts = []
    for checkpoint in (10, 30, 60):
        sample = [x for x in rows if x['scenario']=='continuous' and x['mode']=='baseline'
                  and x['ride_minutes']>=120 and x['checkpoint']==checkpoint]
        counts.append(f"{sum(x['resets_so_far']>0 for x in sample)}/{len(sample)} by {checkpoint} minutes")
    r.paragraph("Unknown-gap resets had already occurred in "+", ".join(counts)+". These resets are part of the frozen replay policy. A real device with a known pause can retain its current pace adjustment. Sudden changes in the example curves can therefore reflect missing-data handling as well as observed pace. Validate this distinction with original timer-event recordings before treating these numbers as device ETA accuracy.")
    r.heading("Largest baseline errors at ten minutes")
    lookup = {x['id']: x for x in rides}
    worst = sorted([x for x in rows if x['scenario']=='continuous' and x['mode']=='baseline' and x['checkpoint']==10 and x['ride_minutes']>=120], key=lambda x: abs(x['predicted_minutes']/x['actual_minutes']-1), reverse=True)[:5]
    r.table(['Ride date', 'Observed hours', 'Actual remaining min', 'Baseline predicted min', 'APE', 'Unknown min'],
        [[lookup[x['ride']]['date'][:10], f"{x['ride_minutes']/60:.2f}", f"{x['actual_minutes']:.0f}", f"{x['predicted_minutes']:.0f}", f"{100*abs(x['predicted_minutes']/x['actual_minutes']-1):.1f}%", f"{lookup[x['ride']]['unknown_minutes']:.0f}"] for x in worst])
    r.paragraph("These cases locate errors; they do not identify fatigue, wind, trail difficulty, or companions as causes. No extra model was fitted after inspecting them.")
    continuous_long = [x for x in rows if x['scenario']=='continuous' and x['mode']=='baseline' and x['checkpoint']==10 and x['ride_minutes']>=120]
    selected = [('Largest ten-minute error', worst[0]['ride']),
                ('Longest evaluation ride', max(continuous_long, key=lambda x: x['ride_minutes'])['ride'])]
    fig, axes = plt.subplots(1, 2, figsize=(11, 4), constrained_layout=True)
    for ax, (label, id) in zip(axes, selected):
        baseline = sorted([x for x in rows if x['scenario']=='continuous' and x['mode']=='baseline' and x['ride']==id], key=lambda x:x['at_minutes'])
        at = [x['at_minutes']/60 for x in baseline]
        ax.plot(at, [x['actual_minutes'] for x in baseline], color='#333333', linestyle='--', label='Actual accepted time left')
        ax.fill_between(at, [x['low_minutes'] for x in baseline], [x['high_minutes'] for x in baseline], color=COLORS['baseline'], alpha=.13, label='Baseline nominal 90% range')
        for mode in LABELS:
            points = sorted([x for x in rows if x['scenario']=='continuous' and x['mode']==mode and x['ride']==id], key=lambda x:x['at_minutes'])
            ax.plot([x['at_minutes']/60 for x in points], [x['predicted_minutes'] for x in points], color=COLORS[mode], label=LABELS[mode])
        ax.set(xlabel='Observed moving hours', ylabel='Remaining observed minutes', title=label+'\n'+lookup[id]['date'][:10])
        ax.grid(alpha=.2); ax.legend(fontsize=7)
    path = output/'examples.svg'; fig.savefig(path); fig.savefig(output/'examples.png', dpi=140); plt.close(fig)
    r.figure(path, 'Diagnostic examples selected by largest baseline error at ten minutes and longest accepted duration. Unknown gaps are omitted from the time axis.')

    r.heading("Reproduction")
    r.paragraph(f"The input plan and runner/test execution manifest were frozen before predictions. Their hashes and all result hashes verify. Timed replay loops took {run['runtime_seconds']/60:.1f} minutes on the host; file loading and block preparation are excluded. This is an offline experiment over many simulated histories, not device runtime evidence. Raw data, per-ride predictions, and this report remain local.")
    r.paragraph("Run from the repository root: python3 host/ride-time-prototype/komoot_replay.py freeze; then the same command with run; then python3 host/ride-time-prototype/komoot_report.py. Replays refuse to overwrite existing runs; reports can be regenerated from verified results.")
    r.paragraph("Two preparation attempts were preserved before the completed execution: one stopped on an MTB-label mismatch, and one was stopped to avoid repeated NPZ decompression. Neither produced predictions. The completed execution used the original plan, cohort, model parameters, and corrected runner frozen before its first forecast.")
    summary.update(protocol_sha256=digest(output/'protocol.json'), execution_sha256=digest(output/'execution.json'),
                   result_hashes=run['hashes'], runtime_seconds=run['runtime_seconds'])
    (output/'summary.json').write_text(json.dumps(summary, indent=2)+"\n")
    r.save(output)
    print(json.dumps(dict(primary=long['10'], paired_change=change, report=str(output/'report.html')), indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=SOURCE)
    parser.add_argument('--output', type=Path, default=OUTPUT)
    args = parser.parse_args()
    report(args.source, args.output)
