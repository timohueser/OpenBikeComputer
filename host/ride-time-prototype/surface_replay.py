"""Frozen regional comparison with common targets and causal observation updates."""

import argparse
import csv
from datetime import datetime, timezone
import gzip
import hashlib
import json
import math
from pathlib import Path
import sqlite3
import time
import zlib

import numpy as np

from data import CACHE, valid_mask
from model import Config, Layout, Learner, Live
from replay import identifier, learn_mask
from surface_data import OUTPUT, cohort
from surface_model import MODELS, correction, features, fit

FIELDS = ("rider", "ride", "sport", "model", "phase", "target_km", "status", "actual_minutes",
          "predicted_minutes", "history_km", "surface_fraction", "path_fraction")


def config():
    return Config(**json.loads(Path(".artifacts/ride-time/config.json").read_text()))


def rides(split):
    with sqlite3.connect(CACHE / "fitrec.sqlite") as db:
        for item in cohort():
            if item["split"] != split:
                continue
            points, blob = db.execute("SELECT points,data FROM rides WHERE ride=?", (item["ride"],)).fetchone()
            values = np.frombuffer(zlib.decompress(blob), dtype="<f4").reshape(4, points-1)
            with gzip.open(OUTPUT / "matches" / f"{item['ride']}.json.gz", "rt") as f:
                matched = json.load(f)
            if len(matched["causal"]) != values.shape[1] or len(matched["offline"]) != values.shape[1]:
                raise ValueError("Feature/interval alignment mismatch")
            yield item, values, matched


def issue(values, start, phase, times, live, history, tags):
    distance = np.r_[0., np.cumsum(values[0], dtype=float)]
    cfg = config()
    for target in cfg.target_km:
        end = int(np.searchsorted(distance, distance[start]+target))
        if end >= len(distance):
            continue
        km = distance[end]-distance[start]
        coverage = {tag: sum(float(values[0, i])*sum(tags[i].get(tag, {}).values())
                             for i in range(start, end))/km for tag in ("surface", "highway")}
        for name in MODELS:
            prediction = float(times[name][start:end].sum()) * (math.exp(float(live[name].u)) if phase else 1.)
            yield dict(model=name, phase=phase, target_km=target, start=start, end=end,
                       predicted_minutes=prediction, history_km=history,
                       surface_fraction=coverage["surface"], path_fraction=coverage["highway"])


def run(split, fitted):
    clock = time.perf_counter()
    cfg = config()
    layout = Layout(cfg, gradient=False)
    previous, history = None, 0.
    output = []
    finish_seconds = []
    for item, values, matched in rides(split):
        if previous != item["user"]:
            previous, history = item["user"], 0.
            learners = {name: Learner(layout) for name in MODELS}
        live = {name: Live(cfg) for name in MODELS}
        initial = layout.initial_log(values[2])
        route_x, past_x = (features(matched[key], item["sport"]) for key in ("offline", "causal"))
        route_times, past_logs = {}, {}
        for name, learner in learners.items():
            learner.reset_ride()
            theta = fitted["coefficients"][name]
            route_times[name] = values[0] * np.exp(initial + correction(route_x, theta) + learner.theta[0])
            past_logs[name] = initial + correction(past_x, theta)
        pending = list(issue(values, 0, 0, route_times, live, history, matched["offline"]))
        valid, learn = valid_mask(values), learn_mask(values, 30)
        established = False
        one = np.ones(1, dtype=np.float32)
        for i in range(values.shape[1]):
            if not established and live["gradient"].accepted_minutes >= cfg.established_minutes:
                pending.extend(issue(values, i, 1, route_times, live, history, matched["offline"]))
                established = True
            for name, learner in learners.items():
                if valid[i]:
                    baseline = float(values[0, i] * np.exp(past_logs[name][i] + learner.theta[0]))
                    live[name].observe(float(values[1, i]), baseline)
                if learn[i]:
                    learner.observe(one, float(past_logs[name][i]), float(values[1, i]/values[0, i]), float(values[0, i]))
        for row in pending:
            start, end = row.pop("start"), row.pop("end")
            accepted = bool(valid[start:end].all())
            row.update(status="scored" if accepted else "censored",
                       actual_minutes=float(values[1, start:end].sum(dtype=float)) if accepted else None,
                       rider=identifier(item["user"]), ride=identifier(item["ride"]), sport=item["sport"])
            output.append(row)
        for learner in learners.values():
            start = time.perf_counter()
            learner.finish()
            finish_seconds.append(time.perf_counter()-start)
            if learner.rejections:
                raise ValueError("Rejected personal update")
        history += float(values[0, learn].sum())
    with (OUTPUT / f"{split}.csv").open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows(output)
    (OUTPUT / f"{split}-runtime.json").write_text(json.dumps(dict(
        runtime_seconds=time.perf_counter()-clock, scalar_array_bytes=Learner(layout).state_bytes,
        shared_coefficient_float32_bytes=4*max(MODELS.values()),
        finish_seconds_median=float(np.median(finish_seconds)), finish_seconds_max=max(finish_seconds)), indent=2)+"\n")
    print(f"{split}: {len(output)//len(MODELS)} common offered targets; "
          f"{sum(r['status']=='scored' for r in output)//len(MODELS)} scored", flush=True)


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def frozen_inputs():
    root = Path(__file__).parent
    files = [root / name for name in ("surface_data.py", "surface_model.py", "surface_replay.py", "model.py",
                                      "replay.py", "data.py", "matching_v2.py", "enrichment_match.py")]
    files += [OUTPUT / "fit.json", OUTPUT / "cohort.json", Path(".artifacts/ride-time/config.json"),
              Path(".artifacts/ride-time-enrichment/ways.json.gz")]
    files += sorted((OUTPUT / "matches").glob("*.json.gz"))
    return {str(p): digest(p) for p in files}


def freeze():
    path = OUTPUT / "frozen.json"
    if path.exists():
        raise SystemExit("Refusing to replace freeze")
    fitted = fit(list(rides("development")), Layout(config(), gradient=False))
    (OUTPUT / "fit.json").write_text(json.dumps(fitted, indent=2) + "\n")
    manifest = dict(frozen_at_utc=datetime.now(timezone.utc).isoformat(), inputs=frozen_inputs(),
                    primary="surface versus bike after 10 accepted minutes",
                    metrics="rider-weighted MAPE; paired rider bootstrap 95% interval; signed bias; p90 APE",
                    selection="All wholly contained rides in [9,56,10,57], original first-60 and overlap policy",
                    history="Regional rides only; one personal scalar across categories",
                    calibration="Diagnostic cohort, no tuning or fitted prediction intervals",
                    route="Recorded future geometry and offline OSM tags proxy a known planned route",
                    live="Past-only forward matching; no future coordinates in observation features",
                    limitations="Earlier global test results were already seen; geography chosen from development; current OSM")
    path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(fitted, indent=2))


def verify():
    frozen = json.loads((OUTPUT / "frozen.json").read_text())
    if frozen["inputs"] != frozen_inputs():
        raise ValueError("Frozen input changed")
    return frozen


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("freeze", "calibration", "test"))
    args = parser.parse_args()
    if args.stage == "freeze":
        freeze()
    else:
        verify()
        run(args.stage, json.loads((OUTPUT / "fit.json").read_text()))
