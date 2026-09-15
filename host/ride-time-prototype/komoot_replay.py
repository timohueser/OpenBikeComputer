"""Frozen chronological replay over accepted Komoot moving sections."""

import argparse
import csv
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import platform
import time

import numpy as np

from endurance import Correction
from final_model import Scalar
from komoot_data import OUTPUT, SOURCE
from komoot_protocol import verify as verify_plan
from long_data import digest
from long_replay import parameters
from model import Intervals, Layout, Live, group_for

ROOT = Path(__file__).resolve().parent
MODES = ("baseline", "gradient")
BUDGETS = (0, 50, 100, 200, 500, 1000)
FIELDS = ("scenario", "mode", "ride", "bike", "history_target_km", "history_km", "history_rides",
          "checkpoint", "at_minutes", "ride_minutes", "actual_minutes", "predicted_minutes",
          "low_minutes", "high_minutes", "theta", "coefficient", "live_log", "resets_so_far")


def blocks(data, cfg, offset):
    """Six rows: km, minutes, initial log pace, learn, uphill weight, live reset."""
    distance, minutes, grade = data["raw"]
    cost = distance*np.exp(Layout(cfg, gradient=False).initial_log(grade)+offset)
    rows, pending = [], []

    def flush():
        if not pending:
            return
        ix = np.asarray(pending)
        d, t = float(distance[ix].sum()), float(minutes[ix].sum())
        learn = bool(data["learn"][ix].all() and d >= .01 and np.ptp(grade[ix]) <= .02)
        g = float(distance[ix] @ grade[ix])/d
        rows.append((d, t, math.log(float(cost[ix].sum())/d), float(learn),
                     float(np.clip(g/.08, 0, 1)), 0.))
        pending.clear()

    accumulated = 0.
    for i in range(len(distance)):
        if data["reset"][i]:
            flush()
            accumulated = 0.
        if data["unknown_minutes"][i] > 0:
            rows.append((0., 0., 0., 0., 0., 1.))
        if distance[i] > 0 and minutes[i] > 0:
            pending.append(i)
            accumulated += distance[i]
        if accumulated >= .02 or data["reset"][i]:
            flush()
            accumulated = 0.
    flush()
    return np.asarray(rows, dtype=float).reshape(-1, 6).T


def new_device(cfg, reference, mode):
    return Scalar(cfg), Correction(mode), Intervals(cfg, reference) if mode == "baseline" else None


def replay(values, cfg, device, train=True, calibrate=True, emit=True):
    learner, correction, ranges = device
    distance, minutes, logs, learn, uphill, reset = values
    beta, theta = correction.beta, learner.theta
    x = uphill if correction.mode == "gradient" else np.zeros(len(distance))
    base = distance*np.exp(logs+theta+beta*x)
    remaining = np.r_[np.cumsum(base[::-1])[::-1], 0.]
    actual = np.r_[np.cumsum(minutes[::-1])[::-1], 0.]
    live, pending, rows, checkpoint, elapsed, resets = Live(cfg), {}, [], 0, 0., 0
    for i in range(len(distance)):
        if reset[i]:
            live = Live(cfg)
            resets += 1
        if distance[i] <= 0:
            continue
        if elapsed >= checkpoint:
            prediction = float(remaining[i])*math.exp(float(live.u))
            phase = int(checkpoint > 0)
            low, high = ranges.predict(phase, prediction) if ranges is not None else (None, None)
            if emit:
                rows.append(dict(checkpoint=checkpoint, at_minutes=elapsed, ride_minutes=float(actual[0]),
                    actual_minutes=float(actual[i]), predicted_minutes=prediction,
                    low_minutes=low, high_minutes=high, theta=theta, coefficient=beta,
                    live_log=float(live.u), resets_so_far=resets))
            if calibrate and ranges is not None:
                pending.setdefault(group_for(cfg, phase, prediction), math.log(float(actual[i])/prediction))
            checkpoint = (int(elapsed/10)+1)*10
        live.observe(float(minutes[i]), float(base[i]))
        if train and learn[i]:
            learner.observe(float(logs[i]+beta*x[i]), float(minutes[i]/distance[i]), float(distance[i]))
        elapsed += float(minutes[i])
    if train:
        mask = learn.astype(bool)
        residual = np.log(minutes[mask]/distance[mask])-logs[mask]-theta
        correction.finish(x[mask], residual, distance[mask], np.zeros(mask.sum(), dtype=int))
        learner.finish()
        if learner.rejections:
            raise ValueError("Rejected persistent update")
    if calibrate and ranges is not None:
        ranges.add_ride(pending)
    return rows


def can_train(ride):
    return ride["moving_minutes"] >= 5 and ride["distance_km"] >= 1


def history_indices(rides, index, budget):
    """Select only prior complete eligible recordings, retaining whole-ride overshoot."""
    selected, km = [], 0.
    for j in range(index-1, -1, -1):
        if km >= budget:
            break
        if can_train(rides[j]):
            selected.append(j)
            km += rides[j]["distance_km"]
    return list(reversed(selected)), km


def execution_inputs(output):
    paths = [ROOT/"komoot_replay.py", ROOT/"tests/test_komoot_replay.py", output/"protocol.json"]
    return {str(p.resolve()): digest(p) for p in paths}


def freeze(source, output):
    verify_plan(source, output)
    manifest = dict(created_at_utc=datetime.now(timezone.utc).isoformat(),
        hashes=execution_inputs(output), python=platform.python_version(), numpy=np.__version__,
        platform=platform.platform(),
        block_feature="Distance-weighted mean grade per uninterrupted block, clipped as grade/8% to [0,1]. Original Correction.finish unchanged.",
        checkpoints="First accepted block boundary at/after each 10 observed minutes. Emit only while accepted route remains. No interpolation of observations.",
        gaps="Flush on every reset mask. Unknown positive active time emits a zero-distance live-reset event. Explicit pauses and stationary intervals flush blocks without resetting live form. Prepared track breaks with positive active time are unknown events.",
        training="All eligible earlier recordings, including accepted spans from incomplete rides. Range updates only on proxy-eligible completed rides.",
        scenarios="Continuous history and paired whole-ride budgets; both models use identical blocks, targets, and history sets.")
    with (output/"execution.json").open("x") as f:
        f.write(json.dumps(manifest, indent=2)+"\n")
    return manifest


def verify(source, output):
    verify_plan(source, output)
    manifest = json.loads((output/"execution.json").read_text())
    if manifest["hashes"] != execution_inputs(output):
        raise ValueError("Frozen execution inputs changed")
    if (manifest["python"], manifest["numpy"]) != (platform.python_version(), np.__version__):
        raise ValueError("Frozen execution software changed")
    return manifest


def configuration():
    cfg, _, reference = parameters()
    offsets = json.loads((ROOT/"results/final-v1.json").read_text())["priors"]["log_offsets"]
    return cfg, {key.lower(): value for key, value in offsets.items()}, reference


def run(source, output):
    verify(source, output)
    with (output/"run-start.json").open("x") as f:
        json.dump(dict(started_at_utc=datetime.now(timezone.utc).isoformat()), f)
    cfg, offsets, reference = configuration()
    plan = json.loads((output/"protocol.json").read_text())
    included = set(plan["cohort"]["warmup"]+plan["cohort"]["evaluation"])
    rides = [r for r in json.loads((output/"rides.json").read_text()) if r["id"] in included]
    targets, long_targets = set(plan["proxy_targets"]), set(plan["long_proxy_targets"])
    values = []
    for r in rides:
        with np.load(output/(r["id"]+".npz")) as archive:
            data = {key: archive[key] for key in archive.files}
        values.append(blocks(data, cfg, offsets[r["bike"]]))
    states, started = [], time.perf_counter()
    with (output/"predictions.csv").open("x", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()

        def process(i, device, mode, scenario, km, count, budget="", emit=False):
            r = rides[i]
            result = replay(values[i], cfg, device, can_train(r), r["proxy_eligible"], emit)
            for row in result:
                writer.writerow(dict(scenario=scenario, mode=mode, ride=r["id"], bike=r["bike"],
                    history_target_km=budget, history_km=km, history_rides=count, **row))

        devices = {m: new_device(cfg, reference, m) for m in MODES}
        km, count = 0., 0
        for i, r in enumerate(rides):
            for mode in MODES:
                process(i, devices[mode], mode, "continuous", km, count, emit=r["id"] in targets)
                learner, correction, _ = devices[mode]
                states.append(dict(mode=mode, ride=r["id"], theta=learner.theta,
                    coefficient=correction.beta, identifiable_updates=correction.updates))
            if can_train(r):
                km, count = km+r["distance_km"], count+1
            if (i+1) % 100 == 0:
                print(f"continuous {i+1}/{len(rides)}; {time.perf_counter()-started:.1f}s", flush=True)
        paired = [i for i, r in enumerate(rides) if r["id"] in long_targets
                  and history_indices(rides, i, 1000)[1] >= 1000]
        history_summary = []
        for n, i in enumerate(paired):
            for budget in BUDGETS:
                prior, km = history_indices(rides, i, budget)
                history_summary.append(dict(ride=rides[i]["id"], budget=budget, km=km, rides=len(prior)))
                for mode in MODES:
                    device = new_device(cfg, reference, mode)
                    for j in prior:
                        process(j, device, mode, "paired", 0, 0)
                    process(i, device, mode, "paired", km, len(prior), budget, True)
            if (n+1) % 5 == 0 or n == len(paired)-1:
                f.flush()
                print(f"paired {n+1}/{len(paired)}; {time.perf_counter()-started:.1f}s", flush=True)
    for name, obj in (("states.json", states), ("history.json", history_summary)):
        with (output/name).open("x") as f:
            f.write(json.dumps(obj, indent=2)+"\n")
    with (output/"run.json").open("x") as f:
        json.dump(dict(runtime_seconds=time.perf_counter()-started, paired_targets=len(paired),
            hashes={n: digest(output/n) for n in ("predictions.csv", "states.json", "history.json", "execution.json")}), f, indent=2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("freeze", "verify", "run"))
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    result = {"freeze": freeze, "verify": verify, "run": run}[args.stage](args.source, args.output)
    if result is not None:
        print(json.dumps({"stage": args.stage, "verified": True}, indent=2))
