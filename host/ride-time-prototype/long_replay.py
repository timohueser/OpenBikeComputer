"""Frozen, chronological new-device pilot on one GoldenCheetah athlete."""

import argparse
import csv
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import time

import numpy as np

from final_model import Scalar
from long_data import ARCHIVE, OUTPUT, digest, prepare
from model import Config, Intervals, Layout, Live, group_for

ROOT = Path(__file__).resolve().parent
BUDGETS = (0, 50, 100, 200, 500, 1000)
FIELDS = ("mode", "ride", "year", "history_target_km", "history_km", "history_rides", "checkpoint",
          "at_minutes", "ride_minutes", "actual_minutes", "predicted_minutes", "low_minutes",
          "high_minutes", "theta", "live_log", "distance_km")


def parameters():
    frozen = json.loads((ROOT / "results/final-v1.json").read_text())
    # Check the estimator against the previous study, not just against this pilot.
    for name in ("model.py", "final_model.py"):
        expected = next(value for path, value in frozen["protocol"]["inputs"].items()
                        if Path(path).name == name)
        if digest(ROOT / name) != expected:
            raise ValueError("Original estimator changed: "+name)
    return (Config(**frozen["config"]), frozen["priors"]["log_offsets"]["other"],
            frozen["reference"]["bike_scalar_live"])


def blocks(raw, cfg, offset):
    """Aggregate positive-motion samples into about 20 m, flushing at stops."""
    distance, minutes, grade = raw
    cost = distance*np.exp(Layout(cfg, gradient=False).initial_log(grade)+offset)
    rows, d, t, q, first = [], 0., 0., 0., 0
    for i in range(len(distance)):
        if minutes[i] > 0:
            if d == 0:
                first = i
            d, t, q = d+distance[i], t+minutes[i], q+cost[i]
        if d > 0 and (d >= .02 or minutes[i] == 0 or i == len(distance)-1):
            learn = d >= .01 and np.ptp(grade[first:i+1]) <= .02
            rows.append((d, t, math.log(q/d), float(learn)))
            d, t, q = 0., 0., 0.
    if not rows:
        raise ValueError("No motion blocks")
    return np.array(rows).T


def new_device(cfg, reference):
    return Scalar(cfg), Intervals(cfg, reference)


def ride_replay(values, cfg, learner, ranges):
    """Forecast before observing a block; commit all persistent changes at ride end."""
    distance, minutes, logs, learn = values
    base = distance*np.exp(logs+learner.theta)
    remaining = np.r_[np.cumsum(base[::-1])[::-1], 0.]
    actual = np.r_[np.cumsum(minutes[::-1])[::-1], 0.]
    live, pending, rows, next_checkpoint = Live(cfg), {}, [], 0
    elapsed = 0.
    for i in range(len(distance)):
        if elapsed >= next_checkpoint:
            phase = int(next_checkpoint > 0)
            prediction = float(remaining[i])*math.exp(float(live.u))
            low, high = ranges.predict(phase, prediction)
            rows.append(dict(checkpoint=next_checkpoint, at_minutes=elapsed,
                ride_minutes=float(actual[0]), actual_minutes=float(actual[i]),
                predicted_minutes=prediction, low_minutes=low, high_minutes=high,
                theta=learner.theta, live_log=float(live.u), distance_km=float(distance.sum())))
            group = group_for(cfg, phase, prediction)
            pending.setdefault(group, math.log(float(actual[i])/prediction))
            next_checkpoint = (int(elapsed/10)+1)*10
        live.observe(float(minutes[i]), float(base[i]))
        if learn[i]:
            learner.observe(float(logs[i]), float(minutes[i]/distance[i]), float(distance[i]))
        elapsed += minutes[i]
    learner.finish()
    if learner.rejections:
        raise ValueError("Rejected scalar observation/update")
    ranges.add_ride(pending)
    return rows


def history_start(rides, index, budget):
    start, km = index, 0.
    while start > 0 and km < budget:
        start -= 1
        km += rides[start]["distance_km"]
    return start, km


def hashes(output):
    files = [ROOT / name for name in ("long_data.py", "long_replay.py", "final_model.py", "model.py", "results/final-v1.json")]
    files += [output / name for name in ("audit.json", "rides.json")]
    files += sorted(output.glob("*.npz"))
    return {str(p): digest(p) for p in files}


def freeze(output):
    parameters()
    path = output / "protocol.json"
    if path.exists():
        raise ValueError("Refusing to replace frozen protocol")
    protocol = dict(created_at_utc=datetime.now(timezone.utc).isoformat(), hashes=hashes(output),
        source_scope="One previously inspected example athlete; descriptive external pilot, not a population test.",
        primary="Continuous chronological replay, empty state at first retained ride; all eligible history rides.",
        annual="Separate reset at the first retained ride of each year; each ride once in this view.",
        paired="Same >=120 minute rides with >=1000 km preceding eligible history; latest whole rides reaching each budget.",
        budgets_km=BUDGETS, paired_overshoot="Include the oldest ride in full; report actual distance; never split a training ride.",
        checkpoints_minutes="Every 10 moving minutes; headline checkpoints 0, 10, 30, 60.",
        bike="All rides use frozen other-bike prior because broad categories are unavailable.",
        motion="Positive distance increments; internal zero-distance runs <=10 s included only in a fixed sensitivity.",
        adaptation="Original Scalar, Live and Intervals unchanged; 20 m observation blocks, flushed at stops; 200 m trailing grade.",
        ranges="Original FitRec references; earliest forecast per phase/duration group updates personal range after each ride.",
        censoring="Reject whole rides with missing data, >30 s gaps, speed >80 km/h, grade >50%, nonmonotonic distance/time.",
        duplicate_policy="GPS/altitude flags required; potential same-day duplicates within 5% distance and elapsed duration removed, prefer finer sampling; then remove overlaps.",
        uncertainty="No rider-bootstrap interval from a single rider; repeated rides/resets are not independent users.")
    path.write_text(json.dumps(protocol, indent=2)+"\n")


def verify(output):
    protocol = json.loads((output / "protocol.json").read_text())
    if protocol["hashes"] != hashes(output):
        raise ValueError("Frozen pilot inputs changed")
    return protocol


def run(output):
    verify(output)
    if (output / "predictions.csv").exists():
        raise ValueError("Evaluation already exists; preserve it before a new run")
    cfg, offset, reference = parameters()
    rides = json.loads((output / "rides.json").read_text())
    primary, alternate = [], []
    for ride in rides:
        with np.load(output / (ride["id"]+".npz")) as data:
            raw = data["raw"]
            primary.append(blocks(raw, cfg, offset))
            raw[1] = data["alternate_minutes"]
            alternate.append(blocks(raw, cfg, offset))
    started = time.perf_counter()
    with (output / "predictions.csv").open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()

        def emit(mode, i, history, count, budget, result):
            for row in result:
                writer.writerow(dict(mode=mode, ride=rides[i]["id"], year=rides[i]["date"][:4],
                    history_target_km=budget, history_km=history, history_rides=count, **row))

        for mode, sequences in (("continuous", primary), ("annual", primary), ("short_stops", alternate)):
            learner, ranges = new_device(cfg, reference)
            history, count, year = 0., 0, None
            for i, ride in enumerate(rides):
                if mode == "annual" and year != ride["date"][:4]:
                    learner, ranges = new_device(cfg, reference)
                    history, count = 0., 0
                year = ride["date"][:4]
                result = ride_replay(sequences[i], cfg, learner, ranges)
                emit(mode, i, history, count, "", result)
                history, count = history+ride["distance_km"], count+1
            print(f"{mode}: {len(rides)} rides; {time.perf_counter()-started:.1f}s", flush=True)

        targets = [i for i, r in enumerate(rides) if r["moving_minutes"] >= 120
                   and history_start(rides, i, 1000)[1] >= 1000]
        for number, i in enumerate(targets):
            for budget in BUDGETS:
                start, history = history_start(rides, i, budget)
                learner, ranges = new_device(cfg, reference)
                for j in range(start, i):
                    ride_replay(primary[j], cfg, learner, ranges)
                emit("paired", i, history, i-start, budget, ride_replay(primary[i], cfg, learner, ranges))
            if number % 5 == 0:
                print(f"paired: {number+1}/{len(targets)} target rides; {time.perf_counter()-started:.1f}s", flush=True)
    (output / "run.json").write_text(json.dumps(dict(runtime_seconds=time.perf_counter()-started,
        paired_target_rides=len(targets), predictions_sha256=digest(output / "predictions.csv")), indent=2)+"\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("prepare", "freeze", "run"))
    parser.add_argument("--archive", type=Path, default=ARCHIVE)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    if args.stage == "prepare":
        print(json.dumps(prepare(args.archive, args.output), indent=2))
    elif args.stage == "freeze":
        freeze(args.output)
    else:
        run(args.output)


if __name__ == "__main__":
    main()
