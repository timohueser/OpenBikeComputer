"""Synthetic robustness cases and host numerical/resource measurements."""

import argparse
from dataclasses import replace
import json
import math
from pathlib import Path
import time
import tracemalloc

import numpy as np
from scipy.optimize import lsq_linear

from data import CACHE, read_rides
from model import Config, Intervals, Layout, Learner, Live
from replay import learn_mask


def train_ride(model, truth, factor=1.0, bike=0, surface=0, grades=None):
    grades = np.tile([-0.03, 0.0, 0.03, 0.08], 20) if grades is None else np.asarray(grades)
    model.reset_ride()
    features = model.layout.features(grades, bike, surface)
    initial = model.layout.initial_log(grades)
    actual = np.exp(np.interp(grades, truth.anchors, np.log(truth.initial_pace))) * factor
    for x, z0, pace in zip(features, initial, actual):
        model.observe(x, float(z0), float(pace), 10 / len(grades))
    model.finish()


def relative_prediction(model, truth, factor=1.0, bike=0, surface=0, grade=0.0):
    x = model.layout.features([grade], bike, surface)[0]
    predicted = math.exp(float(model.layout.initial_log([grade])[0] + x @ model.theta))
    actual = math.exp(float(np.interp(grade, truth.anchors, np.log(truth.initial_pace)))) * factor
    return predicted / actual


def synthetic(cfg):
    cold = {}
    for factor in (0.5, 2.0):
        initial = replace(cfg, initial_pace=tuple(factor * p for p in cfg.initial_pace))
        model = Learner(Layout(initial))
        history = [relative_prediction(model, cfg)]
        for _ in range(30):
            train_ride(model, cfg)
            history.append(relative_prediction(model, cfg))
        cold[str(factor)] = history
    stable = Learner(Layout(cfg))
    for _ in range(20):
        train_ride(stable, cfg)
    before = relative_prediction(stable, cfg)
    train_ride(stable, cfg, 1.5)
    unusual = relative_prediction(stable, cfg) / before - 1
    sustained = [relative_prediction(stable, cfg, 1.3)]
    for _ in range(20):
        train_ride(stable, cfg, 1.3)
        sustained.append(relative_prediction(stable, cfg, 1.3))
    transfer = Learner(Layout(cfg, bikes=2, surfaces=2))
    combinations = [(0, 0, 0), (1, 0, 0), (0, 0, 0.08), (0, 1, 0.08)]
    for ride in range(100):
        bike, surface, grade = combinations[ride % len(combinations)]
        train_ride(transfer, cfg, 1.2 ** bike * 1.35 ** surface, bike, surface, np.full(40, grade))
    transfer_ratio = relative_prediction(transfer, cfg, 1.2 * 1.35, 1, 1, 0.08)
    live = Live(cfg)
    response = []
    for step in range(61):
        response.append(dict(minutes=step * 0.5, multiplier=math.exp(float(live.u))))
        live.observe(0.5, 0.5 / 1.3)
    return dict(cold_start=cold, single_slow_ride_baseline_change=unusual,
                sustained_change=sustained, unseen_bike_surface_climb_ratio=transfer_ratio,
                live_response=response)


def numerical(database, cfg):
    layout = Layout(cfg)
    models = [Learner(layout, dtype) for dtype in (np.float32, np.float64)]
    previous = None
    grid = layout.features(np.linspace(-0.2, 0.2, 81))
    precision, solution, objective, timings = [], [], [], []
    records, failures = 0, 0
    for user, _, _, _, values in read_rides(database, "development", cfg.max_rides):
        if records >= 120:
            break
        if previous != user:
            models = [Learner(layout, dtype) for dtype in (np.float32, np.float64)]
            previous = user
        features = layout.features(values[2])
        initial = layout.initial_log(values[2])
        for model in models:
            model.reset_ride()
        for i in np.flatnonzero(learn_mask(values, 30)):
            for model in models:
                model.observe(features[i], float(initial[i]), float(values[1, i] / values[0, i]), float(values[0, i]))
        problem = models[0].problem()
        if problem is None:
            continue
        _, g, H, lower, upper = problem
        H, g = H.astype(float), g.astype(float)
        L = np.linalg.cholesky(H)
        optimum = lsq_linear(L.T, np.linalg.solve(L, g), bounds=(lower, upper),
                             method="bvls", tol=1e-12, max_iter=1000)
        if not optimum.success:
            failures += 1
        begin = time.perf_counter()
        models[0].finish()
        timings.append(1000 * (time.perf_counter() - begin))
        models[1].finish()
        precision.append(float(np.max(np.abs(np.expm1(grid @ (models[0].theta - models[1].theta))))))
        solution.append(float(np.max(np.abs(np.expm1(grid @ (models[0].theta - optimum.x))))))
        theta = models[0].theta.astype(float)
        best_objective = 0.5 * optimum.x @ H @ optimum.x - g @ optimum.x
        objective.append(float(0.5 * theta @ H @ theta - g @ theta - best_objective))
        records += 1
    return dict(development_rides=records, reference_solver_failures=failures,
                max_float32_vs_float64_pace_difference=max(precision),
                max_fixed_sweeps_vs_reference_pace_difference=max(solution),
                max_objective_gap=max(objective),
                finish_ms_quantiles={str(q): float(np.quantile(timings, q)) for q in (0.5, 0.95, 0.99)})


def resources(cfg):
    reference = [[-0.3, 0.3]] * 6
    interval_bytes = Intervals(cfg, reference).state_bytes
    result = {}
    for bikes, surfaces in ((1, 1), (4, 5)):
        layout = Layout(cfg, bikes=bikes, surfaces=surfaces)
        model = Learner(layout)
        x, initial = layout.features([0.02])[0], float(layout.initial_log([0.02])[0])
        live = Live(cfg)
        begin = time.perf_counter()
        for _ in range(5000):
            model.observe(x, initial, math.exp(initial), 0.1)
            live.observe(0.3, 0.3)
        per_observation = (time.perf_counter() - begin) / 5000 * 1e6
        tracemalloc.start()
        probe = Learner(Layout(cfg, bikes=bikes, surfaces=surfaces))
        for _ in range(100):
            probe.observe(x, initial, math.exp(initial), 0.1)
        _, during_peak = tracemalloc.get_traced_memory()
        probe.finish()
        _, finish_peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        result[str(layout.size)] = dict(learner_array_bytes=model.state_bytes,
                                       interval_array_bytes=interval_bytes, live_scalar_bytes=8,
                                       subtotal_array_bytes=model.state_bytes + interval_bytes + 8,
                                       observe_plus_live_microseconds=per_observation,
                                       python_traced_during_ride_peak_bytes=during_peak,
                                       python_traced_including_finish_peak_bytes=finish_peak)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(".artifacts/ride-time"))
    parser.add_argument("--database", type=Path, default=CACHE / "fitrec.sqlite")
    args = parser.parse_args()
    values = json.loads((args.output / "config.json").read_text())
    for key in ("anchors", "initial_pace", "duration_boundaries", "target_km"):
        values[key] = tuple(values[key])
    cfg = Config(**values)
    result = dict(synthetic=synthetic(cfg), numerical=numerical(args.database, cfg), resources=resources(cfg))
    (args.output / "diagnostics.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "synthetic"}, indent=2))
