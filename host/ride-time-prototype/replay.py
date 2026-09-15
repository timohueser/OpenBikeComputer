"""Chronological FitRec replay with prospective route-distance targets."""

import argparse
from collections import Counter
from dataclasses import asdict, replace
import csv
import hashlib
import json
import math
from pathlib import Path
import time

import numpy as np

from data import CACHE, read_rides, valid_mask
from model import Config, Intervals, Layout, Learner, Live, group_for

MODELS = ("fixed_pace", "fixed_grade", "personal_scale", "personal_grade", "scalar_live", "grade_live")
FIELDS = ("rider", "ride", "sport", "model", "phase", "target_km", "distance_km", "history_km", "ride_number",
          "group", "calibration_slot", "status", "actual_minutes", "predicted_minutes", "low_minutes", "high_minutes",
          "unsupported_fraction")


def identifier(value):
    return hashlib.sha256(f"obc-report:{value}".encode()).hexdigest()[:16]


def learn_mask(values, cap):
    grade_change = np.abs(np.diff(np.r_[values[2, 0], values[2]]))
    return valid_mask(values, cap) & (values[0] >= 0.01) & (grade_change <= 0.02)


def fit_initial(database, cfg):
    """Each development rider contributes at most one median per anchor."""
    anchors = np.array(cfg.anchors)
    medians = [[] for _ in anchors]
    per_user = [[] for _ in anchors]
    previous = None
    selection = Counter()

    def flush():
        for j, parts in enumerate(per_user):
            if parts:
                joined = np.concatenate(parts)
                if len(joined) >= 20:
                    medians[j].append(float(np.median(joined)))

    for user, _, _, _, values in read_rides(database, "development", cfg.max_rides, selection):
        if user != previous:
            if previous is not None:
                flush()
            per_user = [[] for _ in anchors]
            previous = user
        valid = learn_mask(values, 30)
        grade = values[2, valid]
        logs = np.log(values[1, valid] / values[0, valid])
        nearest = np.argmin(np.abs(grade[:, None] - anchors[None, :]), axis=1)
        for j in range(len(anchors)):
            per_user[j].append(logs[nearest == j])
    flush()
    result = list(cfg.initial_pace)
    for j, observations in enumerate(medians):
        if len(observations) >= 10:
            result[j] = float(np.clip(np.exp(np.median(observations)), 0.75, 30))
    return replace(cfg, initial_pace=tuple(result)), {
        "source": "Development riders only; median of rider median interval log-paces in nearest-anchor bins",
        "minimum_observations_per_rider_anchor": 20,
        "minimum_riders_per_anchor": 10,
        "riders_per_anchor": list(map(len, medians)),
        "fallback_pace": list(cfg.initial_pace),
        "selection": dict(selection),
    }


def forecast_rows(cfg, values, start, phase, baseline_times, multipliers, reference, selected):
    """Outcomes are evaluated offline; none enters live state in this function."""
    distance, elapsed, grade, _ = values
    cumulative = np.r_[0.0, np.cumsum(distance, dtype=np.float64)]
    prefix = {name: np.r_[0.0, np.cumsum(times, dtype=np.float64)] for name, times in baseline_times.items()}
    for target in cfg.target_km:
        end = int(np.searchsorted(cumulative, cumulative[start] + target))
        if end >= len(cumulative):
            continue
        for model in MODELS:
            base = "personal_scale" if model == "scalar_live" else "personal_grade" if model == "grade_live" else model
            predicted = float((prefix[base][end] - prefix[base][start]) * multipliers.get(model, 1.0))
            if predicted <= 0 or not math.isfinite(predicted):
                raise ValueError("Invalid prediction")
            group = group_for(cfg, phase, predicted)
            slot = group not in selected[model]
            selected[model].add(group)
            low, high = reference[model].predict(phase, predicted) if reference else (None, None)
            yield dict(model=model, phase=phase, target_km=target, start=start, end=end,
                       distance_km=float(cumulative[end] - cumulative[start]), group=group,
                       calibration_slot=int(slot), predicted_minutes=predicted,
                       low_minutes=low, high_minutes=high,
                       unsupported_fraction=float(np.sum(distance[start:end] * (np.abs(grade[start:end]) > 0.2))
                                                  / (cumulative[end] - cumulative[start])))


def run_split(database, cfg, split, output, reference=None, cap=30, max_users=None):
    clock = time.perf_counter()
    layout, scalar_layout = Layout(cfg), Layout(cfg, gradient=False)
    stats, users = Counter(), set()
    previous, history_km, ride_number = None, 0.0, 0
    with output.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for user, ride, _, sport, values in read_rides(database, split, cfg.max_rides, stats):
            if user != previous:
                if max_users and len(users) >= max_users:
                    break
                users.add(user)
                previous, history_km, ride_number = user, 0.0, 0
                learner, scalar = Learner(layout), Learner(scalar_layout)
                ranges = {name: Intervals(cfg, reference[name]) for name in MODELS} if reference else None
            ride_number += 1
            learner.reset_ride()
            scalar.reset_ride()
            live, scalar_live = Live(cfg), Live(cfg)
            distance, elapsed, grades, _ = values
            valid, learn = valid_mask(values, cap), learn_mask(values, cap)
            initial = layout.initial_log(grades)
            features = layout.features(grades)
            baseline_times = {
                "fixed_pace": distance * cfg.initial_pace[layout.reference_anchor],
                "fixed_grade": distance * np.exp(initial),
                "personal_scale": distance * np.exp(initial + scalar.theta[0]),
                "personal_grade": distance * np.exp(initial + features @ learner.theta),
            }
            selected = {name: set() for name in MODELS}
            pending = list(forecast_rows(cfg, values, 0, 0, baseline_times, {}, ranges, selected))
            established = False
            one = np.ones(1, dtype=np.float32)
            for i in range(len(distance)):
                # Predict at this boundary before consuming interval i or its outcome.
                if not established and live.accepted_minutes >= cfg.established_minutes:
                    pending.extend(forecast_rows(cfg, values, i, 1, baseline_times,
                                                {"grade_live": math.exp(float(live.u)),
                                                 "scalar_live": math.exp(float(scalar_live.u))}, ranges, selected))
                    established = True
                if valid[i]:
                    actual = float(elapsed[i])
                    live.observe(actual, float(baseline_times["personal_grade"][i]))
                    scalar_live.observe(actual, float(baseline_times["personal_scale"][i]))
                if learn[i]:
                    pace = float(elapsed[i] / distance[i])
                    learner.observe(features[i], float(initial[i]), pace, float(distance[i]))
                    scalar.observe(one, float(initial[i]), pace, float(distance[i]))
            completed = {name: {} for name in MODELS}
            for item in pending:
                start, end = item.pop("start"), item.pop("end")
                acceptable = bool(valid[start:end].all())
                if acceptable:
                    actual = float(elapsed[start:end].astype(float).sum())
                    item.update(status="scored", actual_minutes=actual)
                    if item["calibration_slot"]:
                        completed[item["model"]][item["group"]] = math.log(actual / item["predicted_minutes"])
                else:
                    item.update(status="censored", actual_minutes=None)
                item.update(rider=identifier(user), ride=identifier(ride), sport=sport, history_km=history_km,
                            ride_number=ride_number)
                writer.writerow(item)
                stats[item["status"]] += 1
            before_rejections = learner.rejections + scalar.rejections
            update_start = time.perf_counter()
            learner.finish()
            scalar.finish()
            stats["two_learner_finish_seconds"] += time.perf_counter() - update_start
            stats["rejected_baseline_updates"] += learner.rejections + scalar.rejections - before_rejections
            if ranges:
                for name in MODELS:
                    ranges[name].add_ride(completed[name])
            history_km += float(learner.nw[1])
            stats["rides"] += 1
            stats["intervals"] += len(distance)
            stats["accepted_intervals"] += int(valid.sum())
            stats["learning_intervals"] += int(learn.sum())
            stats["learning_km"] += float(learner.nw[1])
            if stats["rides"] % 1000 == 0:
                print(f"{split} {cap}s: {stats['rides']:,} rides, {len(users)} riders, "
                      f"{time.perf_counter() - clock:.0f}s", flush=True)
    stats["users"] = len(users)
    stats["runtime_seconds"] = time.perf_counter() - clock
    return dict(stats)


def build_reference(path, cfg):
    errors = {name: [[] for _ in range(6)] for name in MODELS}
    riders = {name: [set() for _ in range(6)] for name in MODELS}
    with path.open() as f:
        for row in csv.DictReader(f):
            if row["status"] != "scored" or row["calibration_slot"] != "1":
                continue
            name, group = row["model"], int(row["group"])
            errors[name][group].append(math.log(float(row["actual_minutes"]) / float(row["predicted_minutes"])))
            riders[name][group].add(row["rider"])
    result, diagnostics = {}, {}
    probabilities = (np.arange(cfg.reference_knots) + 0.5) / cfg.reference_knots
    for name in MODELS:
        pooled = [value for group in errors[name] for value in group]
        if not pooled:
            raise ValueError(f"No calibration outcomes for {name}")
        result[name], diagnostics[name] = [], []
        for group in range(6):
            available = errors[name][group]
            fallback = not available
            reference = pooled if fallback else available
            result[name].append(np.quantile(reference, probabilities).tolist())
            diagnostics[name].append(dict(observations=len(available), riders=len(riders[name][group]),
                                          fallback="pooled_same_model" if fallback else None))
    return result, diagnostics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("configure", "calibrate", "evaluate"))
    parser.add_argument("--database", type=Path, default=CACHE / "fitrec.sqlite")
    parser.add_argument("--output", type=Path, default=Path(".artifacts/ride-time"))
    parser.add_argument("--cap", type=int, default=30)
    parser.add_argument("--max-users", type=int)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    config_path = args.output / "config.json"
    if args.stage == "configure":
        cfg, initial_audit = fit_initial(args.database, Config())
        config_path.write_text(json.dumps(asdict(cfg), indent=2) + "\n")
        (args.output / "initial-curve.json").write_text(json.dumps(initial_audit, indent=2) + "\n")
        print(config_path.read_text())
        return
    values = json.loads(config_path.read_text())
    for key in ("anchors", "initial_pace", "duration_boundaries", "target_km"):
        values[key] = tuple(values[key])
    cfg = Config(**values)
    if args.stage == "calibrate":
        path = args.output / "calibration.csv"
        stats = run_split(args.database, cfg, "calibration", path, max_users=args.max_users)
        reference, diagnostics = build_reference(path, cfg)
        (args.output / "reference.json").write_text(json.dumps(reference, indent=2) + "\n")
        (args.output / "calibration-summary.json").write_text(json.dumps(dict(stats=stats, groups=diagnostics), indent=2) + "\n")
    else:
        reference = json.loads((args.output / "reference.json").read_text())
        stats = run_split(args.database, cfg, "test", args.output / f"test-{args.cap}s.csv",
                          reference, args.cap, args.max_users)
        (args.output / f"test-{args.cap}s-summary.json").write_text(json.dumps(stats, indent=2) + "\n")
    print(json.dumps(stats, indent=2))


if __name__ == "__main__":
    main()
