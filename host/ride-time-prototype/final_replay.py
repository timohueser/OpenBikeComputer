"""Final scalar-model consistency check on the existing full FitRec split."""

import argparse
from collections import Counter
import csv
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import time

import numpy as np
from scipy.optimize import lsq_linear

from data import CACHE, read_rides, valid_mask
from final_model import Scalar
from model import Config, Intervals, Layout, Live, group_for
from replay import FIELDS, identifier, learn_mask

OUTPUT = Path(".artifacts/ride-time-final")
MODELS = ("scalar_live", "bike_scalar_live")


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def config():
    return Config(**json.loads((OUTPUT / "config.json").read_text()))


def fit_priors(cfg):
    """Same bounded bike fit as the regional study, with scalar ride summaries."""
    layout = Layout(cfg, gradient=False)
    rows = []
    for user, _, _, sport, values in read_rides(CACHE / "fitrec.sqlite", "development", cfg.max_rides):
        mask = learn_mask(values, 30)
        if not mask.any():
            continue
        distance = values[0, mask].astype(float)
        residual = np.clip(np.log(values[1, mask]/values[0, mask])-layout.initial_log(values[2, mask]),
                           -math.log(3), math.log(3))
        rows.append((user, sport == "mountain bike", float(distance @ residual/distance.sum())))
    counts = Counter(user for user, *_ in rows)
    x = np.array([[1., mtb] for _, mtb, _ in rows])
    y = np.array([mean for _, _, mean in rows])
    weights = np.sqrt([.25/counts[user] for user, *_ in rows])
    design = np.vstack([weights[:, None]*x, np.diag(np.sqrt([.1, .25]))])
    target = np.r_[weights*y, 0., 0.]
    bounds = np.log([1.5, 2.])
    solved = lsq_linear(design, target, bounds=(-bounds, bounds), tol=1e-10)
    if not solved.success or not np.isfinite(solved.x).all():
        raise ValueError("Bike prior fit failed")
    offset, mtb = solved.x
    return dict(log_offsets={"other": float(offset), "MTB": float(offset+mtb)},
                coefficients=solved.x.tolist(), riders=len(counts), rides=len(rows),
                bike_riders={name: len({u for u, b, _ in rows if b == flag}) for name, flag in (("MTB", True), ("other", False))},
                policy=dict(mean_weight=.25, ridge=[.1, .25], coefficient_bounds=bounds.tolist(), residual_clip=math.log(3)))


def issue(cfg, values, start, phase, baseline, live, ranges, selected):
    distance = np.r_[0., values[0].cumsum(dtype=float)]
    for target in cfg.target_km:
        end = int(np.searchsorted(distance, distance[start]+target))
        if end >= len(distance):
            continue
        for name in MODELS:
            predicted = float(baseline[name][start:end].sum(dtype=float)) * (math.exp(float(live[name].u)) if phase else 1.)
            if not math.isfinite(predicted) or predicted <= 0:
                raise ValueError("Invalid forecast")
            group = group_for(cfg, phase, predicted)
            slot = int(group not in selected[name])
            selected[name].add(group)
            low, high = ranges[name].predict(phase, predicted) if ranges else (None, None)
            yield dict(model=name, phase=phase, target_km=target, start=start, end=end,
                       group=group, calibration_slot=slot, predicted_minutes=predicted,
                       low_minutes=low, high_minutes=high, distance_km=float(distance[end]-distance[start]),
                       unsupported_fraction=float(values[0, start:end] @ (np.abs(values[2, start:end]) > .2)
                                                  / (distance[end]-distance[start])))


def run(split, cfg, priors, reference=None):
    clock = time.perf_counter()
    layout = Layout(cfg, gradient=False)
    stats = Counter()
    previous, history, ride_number = None, 0., 0
    finish_times = []
    with (OUTPUT / f"{split}.csv").open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for user, ride, _, sport, values in read_rides(CACHE / "fitrec.sqlite", split, cfg.max_rides, stats):
            if user != previous:
                previous, history, ride_number = user, 0., 0
                stats["riders"] += 1
                learners = {name: Scalar(cfg) for name in MODELS}
                ranges = {name: Intervals(cfg, reference[name]) for name in MODELS} if reference else None
            ride_number += 1
            valid, learn = valid_mask(values), learn_mask(values, 30)
            initial = layout.initial_log(values[2])
            offset = priors["log_offsets"]["MTB" if sport == "mountain bike" else "other"]
            logs = {"scalar_live": initial, "bike_scalar_live": initial+offset}
            baseline = {name: values[0]*np.exp(logs[name]+learner.theta) for name, learner in learners.items()}
            live = {name: Live(cfg) for name in MODELS}
            selected = {name: set() for name in MODELS}
            pending = list(issue(cfg, values, 0, 0, baseline, live, ranges, selected))
            established = False
            for i in range(values.shape[1]):
                if not established and live["scalar_live"].accepted_minutes >= cfg.established_minutes:
                    pending.extend(issue(cfg, values, i, 1, baseline, live, ranges, selected))
                    established = True
                for name, learner in learners.items():
                    if valid[i]:
                        live[name].observe(float(values[1, i]), float(baseline[name][i]))
                    if learn[i]:
                        learner.observe(float(logs[name][i]), float(values[1, i]/values[0, i]), float(values[0, i]))
            completed = {name: {} for name in MODELS}
            for row in pending:
                start, end = row.pop("start"), row.pop("end")
                accepted = bool(valid[start:end].all())
                actual = float(values[1, start:end].sum(dtype=float)) if accepted else None
                row.update(rider=identifier(user), ride=identifier(ride), sport=sport, history_km=history,
                           ride_number=ride_number, status="scored" if accepted else "censored", actual_minutes=actual)
                writer.writerow(row)
                stats[row["status"]] += 1
                if accepted and row["calibration_slot"]:
                    completed[row["model"]][row["group"]] = math.log(actual/row["predicted_minutes"])
            for name, learner in learners.items():
                started = time.perf_counter()
                learner.finish()
                finish_times.append(time.perf_counter()-started)
                if learner.rejections:
                    raise ValueError("Rejected scalar observation or update")
                if ranges:
                    ranges[name].add_ride(completed[name])
            history += float(values[0, learn].sum(dtype=float))
            stats["rides"] += 1
            stats["learning_km"] += float(values[0, learn].sum(dtype=float))
            stats["accepted_intervals"] += int(valid.sum())
            if stats["rides"] % 1000 == 0:
                print(f"{split}: {stats['rides']:,} rides; {time.perf_counter()-clock:.0f}s", flush=True)
    stats.update(runtime_seconds=time.perf_counter()-clock, finish_median_ms=1000*float(np.median(finish_times)),
                 finish_max_ms=1000*max(finish_times), rejected_updates=0)
    (OUTPUT / f"{split}-summary.json").write_text(json.dumps(dict(stats), indent=2)+"\n")
    return dict(stats)


def reference(cfg):
    errors = {name: [[] for _ in range(6)] for name in MODELS}
    users = {name: [set() for _ in range(6)] for name in MODELS}
    with (OUTPUT / "calibration.csv").open() as f:
        for row in csv.DictReader(f):
            if row["status"] != "scored" or row["calibration_slot"] != "1":
                continue
            name, group = row["model"], int(row["group"])
            errors[name][group].append(math.log(float(row["actual_minutes"])/float(row["predicted_minutes"])))
            users[name][group].add(row["rider"])
    result, support = {}, {}
    probabilities = (np.arange(cfg.reference_knots)+.5)/cfg.reference_knots
    for name in MODELS:
        pooled = [e for group in errors[name] for e in group]
        if not pooled:
            raise ValueError("No calibration outcomes")
        result[name] = [np.quantile(group or pooled, probabilities).tolist() for group in errors[name]]
        support[name] = [dict(observations=len(group), riders=len(users[name][i]), pooled_fallback=not bool(group))
                         for i, group in enumerate(errors[name])]
    return result, support


def inputs():
    root = Path(__file__).parent
    paths = [root / p for p in ("final_model.py", "final_replay.py", "model.py", "replay.py", "data.py")]
    paths += [OUTPUT / "config.json", OUTPUT / "priors.json"]
    return {str(path): digest(path) for path in paths}


def verify():
    protocol = json.loads((OUTPUT / "protocol.json").read_text())
    if protocol["inputs"] != inputs():
        raise ValueError("Frozen estimator or configuration changed")
    if protocol["database_sha256"] != digest(CACHE / "fitrec.sqlite"):
        raise ValueError("Frozen interval dataset changed")
    return protocol


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("configure", "calibrate", "freeze", "evaluate"))
    args = parser.parse_args()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    if args.stage == "configure":
        if (OUTPUT / "protocol.json").exists():
            raise SystemExit("Refusing to replace protocol")
        (OUTPUT / "config.json").write_bytes(Path(".artifacts/ride-time/config.json").read_bytes())
        priors = fit_priors(config())
        (OUTPUT / "priors.json").write_text(json.dumps(priors, indent=2)+"\n")
        protocol = dict(created_at_utc=datetime.now(timezone.utc).isoformat(), inputs=inputs(),
            database_sha256=digest(CACHE / "fitrec.sqlite"),
            primary="bike_scalar_live; all existing test riders; same first-60 and overlap policy",
            comparisons="scalar_live; original grade curve without bike defaults",
            history_bands="zero; 0<km<50; 50<=km<200; km>=200; before current ride",
            intervals="six phase/duration groups; 64 reference knots; 32 personal errors; 32 reference weight",
            interpretation="Consistency check on previously inspected test riders. No tuning after evaluation.")
        (OUTPUT / "protocol.json").write_text(json.dumps(protocol, indent=2)+"\n")
        print(json.dumps(priors, indent=2))
        return
    verify()
    cfg, priors = config(), json.loads((OUTPUT / "priors.json").read_text())
    if args.stage == "calibrate":
        if (OUTPUT / "frozen.json").exists():
            raise SystemExit("Calibration already frozen")
        print(json.dumps(run("calibration", cfg, priors), indent=2))
        ref, support = reference(cfg)
        (OUTPUT / "reference.json").write_text(json.dumps(ref, indent=2)+"\n")
        (OUTPUT / "reference-support.json").write_text(json.dumps(support, indent=2)+"\n")
    elif args.stage == "freeze":
        if (OUTPUT / "frozen.json").exists():
            raise SystemExit("Refusing to replace freeze")
        frozen = dict(created_at_utc=datetime.now(timezone.utc).isoformat(),
                      hashes={name: digest(OUTPUT / name) for name in
                              ("protocol.json", "calibration.csv", "reference.json", "reference-support.json")})
        (OUTPUT / "frozen.json").write_text(json.dumps(frozen, indent=2)+"\n")
    else:
        frozen = json.loads((OUTPUT / "frozen.json").read_text())
        if any(digest(OUTPUT / p) != value for p, value in frozen["hashes"].items()):
            raise ValueError("Frozen calibration changed")
        print(json.dumps(run("test", cfg, priors, json.loads((OUTPUT / "reference.json").read_text())), indent=2))


if __name__ == "__main__":
    main()
