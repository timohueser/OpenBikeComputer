"""Exploratory bounded gradient, sustained-climb, and duration corrections."""

import argparse
import csv
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import time

import numpy as np

from final_model import Scalar
from long_data import OUTPUT as SOURCE, digest
from long_replay import blocks, parameters, verify as verify_source
from model import Live

OUTPUT = Path(".artifacts/ride-time-endurance")
ROOT = Path(__file__).resolve().parent
MODES = ("baseline", "gradient", "sustained", "duration")
FIELDS = ("mode", "ride", "checkpoint", "at_minutes", "ride_minutes", "actual_minutes",
          "predicted_minutes", "history_km", "theta", "coefficient", "live_log")
LIMIT = math.log(1.5)
STEP = .03
RIDGE = .1


def duration_shape(minutes):
    return np.clip((np.asarray(minutes)-120)/240, 0., 1.)


def features(raw, cfg, offset):
    values = blocks(raw, cfg, offset)
    distance = values[0]
    centers = np.cumsum(distance)-distance/2
    grade = np.interp(centers, np.cumsum(raw[0])-raw[0]/2, raw[2])
    uphill = np.clip(grade/.08, 0., 1.)
    gain, interruption, sustained = 0., 0., np.zeros(len(grade))
    for i, (d, g) in enumerate(zip(distance, grade)):
        if g > .02:
            gain += d*1000*g
            interruption = 0.
        else:
            interruption += d
            if interruption >= .2:
                gain = 0.
        sustained[i] = uphill[i]*min(gain/300, 1.)
    groups = np.searchsorted((np.asarray(cfg.anchors[:-1])+cfg.anchors[1:])/2, grade)
    return values, uphill, sustained, groups


def centered_moments(x, y, weights, groups):
    """Remove each ride/grade group's mean before fitting a single slope."""
    xx, xy = 0., 0.
    for group in np.unique(groups):
        m = groups == group
        w = weights[m]
        if w.sum() <= 0:
            continue
        dx = x[m]-np.average(x[m], weights=w)
        dy = y[m]-np.average(y[m], weights=w)
        xx += float(w @ (dx*dx))
        xy += float(w @ (dx*dy))
    return xx, xy


class Correction:
    def __init__(self, mode):
        self.mode = mode
        self.beta, self.A, self.h = 0., 0., 0.
        self.updates = 0

    def finish(self, x, residual, distance, groups):
        if self.mode == "baseline":
            return
        if self.mode == "gradient":
            groups = np.zeros(len(groups), dtype=int)
        xx, xy = centered_moments(x, np.clip(residual, -math.log(2), math.log(2)), distance, groups)
        weight = float(distance.sum())
        if weight <= 0 or xx/weight < 1e-6:
            return
        evidence = min(weight/10, 1.)
        rho = 2**(-evidence/20)
        self.A = rho*self.A + .25*evidence*xx/weight
        self.h = rho*self.h + .25*evidence*xy/weight
        lower = 0. if self.mode == "duration" else -LIMIT
        self.beta = float(np.clip(self.h/(self.A+RIDGE),
            max(lower, self.beta-STEP*evidence), min(LIMIT, self.beta+STEP*evidence)))
        if not all(map(math.isfinite, (self.beta, self.A, self.h))):
            raise ValueError("Nonfinite correction")
        self.updates += 1


def predict(base, x, mode, beta, live_log, elapsed):
    if mode != "duration" or beta == 0:
        return float(np.sum(base*np.exp(beta*x)))*math.exp(live_log)
    # Future fatigue uses predicted arrival time, never recorded future time.
    total = 0.
    factor = math.exp(live_log)
    for initial in base:
        exposure = min(max((elapsed+total-120)/240, 0.), 1.)
        total += float(initial)*factor*math.exp(beta*exposure)
    return total


def replay(data, cfg, learner, correction):
    values, uphill, sustained, groups = data
    distance, minutes, logs, learn = values
    at = np.r_[0., minutes.cumsum()]
    actual = np.r_[minutes[::-1].cumsum()[::-1], 0.]
    mode, beta = correction.mode, correction.beta
    x = (duration_shape(at[:-1]) if mode == "duration" else
         sustained if mode == "sustained" else uphill if mode == "gradient" else np.zeros(len(distance)))
    base = distance*np.exp(logs+learner.theta)
    observed_base = base*np.exp(beta*x)
    live, rows, checkpoint = Live(cfg), [], 0
    for i in range(len(distance)):
        if at[i] >= checkpoint:
            prediction = predict(base[i:], x[i:], mode, beta, float(live.u), float(at[i]))
            rows.append(dict(checkpoint=checkpoint, at_minutes=float(at[i]), ride_minutes=float(actual[0]),
                actual_minutes=float(actual[i]), predicted_minutes=prediction, theta=learner.theta,
                coefficient=beta, live_log=float(live.u)))
            checkpoint = (int(at[i]/10)+1)*10
        live.observe(float(minutes[i]), float(observed_base[i]))
        if learn[i]:
            learner.observe(float(logs[i]+beta*x[i]), float(minutes[i]/distance[i]), float(distance[i]))
    mask = learn.astype(bool)
    # Both persistent updates use the pre-ride parameters. No live residual enters fitting.
    residual = np.log(minutes[mask]/distance[mask])-logs[mask]-learner.theta
    correction.finish(x[mask], residual, distance[mask], groups[mask])
    learner.finish()
    if learner.rejections:
        raise ValueError("Rejected scalar update")
    return rows, dict(coefficient=correction.beta, identifiable_updates=correction.updates,
        feature_km=float(distance @ (x > .01)), saturated_km=float(distance @ (x >= .99)),
        within_group_variance=centered_moments(x[mask], residual, distance[mask],
            np.zeros(mask.sum(), dtype=int) if mode == "gradient" else groups[mask])[0])


def inputs():
    paths = [ROOT / "endurance.py", ROOT / "final_model.py", ROOT / "model.py",
             ROOT / "long_replay.py", ROOT / "results/final-v1.json", SOURCE / "protocol.json"]
    return {str(p): digest(p) for p in paths}


def freeze(output):
    verify_source(SOURCE)
    parameters()
    output.mkdir(parents=True, exist_ok=True)
    path = output / "protocol.json"
    if path.exists():
        raise ValueError("Protocol already exists")
    path.write_text(json.dumps(dict(created_at_utc=datetime.now(timezone.utc).isoformat(), inputs=inputs(),
        scope="Exploratory comparison on the previously inspected single athlete. Not a new held-out test.",
        baseline="Unchanged frozen scalar/live model; same prepared whole-ride cohort and 20 m blocks.",
        gradient="One coefficient times clip(gradient/8%,0,1). Within-ride centered regression.",
        sustained="Uphill weight times clip(accumulated ascent/300 m,0,1); ascent accumulates above 2%, resets after 200 m non-climbing distance.",
        duration="One coefficient times clip((moving minutes-120)/240,0,1); zero prior; nonnegative coefficient.",
        learning="Duration and sustained effects are centered within ride and nearest gradient-anchor group. Eligible baseline-learning blocks only.",
        bounds=dict(maximum_log_correction=LIMIT, maximum_step=STEP, ridge=RIDGE,
                    history_half_evidence=20, ride_cap_km=10, eta=.25, minimum_feature_variance=1e-6),
        forecast="Known future profile for gradient/sustained; forward integration with predicted future moving time for duration. Live residual removes modeled current fatigue.",
        analysis="All long rides and all except longest; departure/10/30/60-minute checkpoints; descriptive paired errors, no population confidence interval.",
        uncertainty="Point predictions only; old range references cannot establish coverage for new variants.",
        no_tuning="No parameter search, combined model, or retrospective same-ride fitting."), indent=2)+"\n")


def verify(output):
    verify_source(SOURCE)
    protocol = json.loads((output / "protocol.json").read_text())
    if protocol["inputs"] != inputs():
        raise ValueError("Frozen extension inputs changed")
    return protocol


def run(output):
    verify(output)
    if (output / "predictions.csv").exists():
        raise ValueError("Predictions already exist")
    cfg, offset, _ = parameters()
    rides = json.loads((SOURCE / "rides.json").read_text())
    learners = {mode: Scalar(cfg) for mode in MODES}
    corrections = {mode: Correction(mode) for mode in MODES}
    history, states, started = 0., [], time.perf_counter()
    with (output / "predictions.csv").open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for number, ride in enumerate(rides):
            with np.load(SOURCE / (ride["id"]+".npz")) as archive:
                data = features(archive["raw"], cfg, offset)
            for mode in MODES:
                rows, state = replay(data, cfg, learners[mode], corrections[mode])
                for row in rows:
                    writer.writerow(dict(mode=mode, ride=ride["id"], history_km=history, **row))
                states.append(dict(mode=mode, ride=ride["id"], **state))
            history += ride["distance_km"]
            if number % 50 == 0:
                print(f"{number+1}/{len(rides)} rides; {time.perf_counter()-started:.1f}s", flush=True)
    (output / "states.json").write_text(json.dumps(states, indent=2)+"\n")
    (output / "run.json").write_text(json.dumps(dict(runtime_seconds=time.perf_counter()-started,
        predictions_sha256=digest(output / "predictions.csv"), states_sha256=digest(output / "states.json")), indent=2)+"\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("freeze", "run"))
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    freeze(args.output) if args.stage == "freeze" else run(args.output)
