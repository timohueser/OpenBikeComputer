"""Conservative GoldenCheetah archive adapter for full-ride replay."""

from collections import Counter
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import zipfile

import numpy as np

SOURCE = "https://raw.githubusercontent.com/GoldenCheetah/OpenData/59a0062c5e79c7e9a6751a3fff972fdb0bdb3347/examples/033874ce-e20d-44ba-9cc9-125030b6662f.zip"
SOURCE_SHA = "9f3b5a8667148a05203d9f3139422a5e1d9e40d4291124313b124289e10e409f"
ARCHIVE = Path.home() / ".cache/openbikecomputer/ride-time/goldencheetah/example.zip"
OUTPUT = Path(".artifacts/ride-time-long")


def digest(path):
    with Path(path).open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def match_metadata(name, records):
    """Require a unique same-day match; source filenames can use local time."""
    date = datetime.strptime(name, "%Y_%m_%d_%H_%M_%S.csv")
    matches = [r for r in records if r["date"][:10] == date.strftime("%Y/%m/%d")
               and r["date"][14:19] == date.strftime("%M:%S")]
    return matches[0] if len(matches) == 1 else None


def intervals(seconds, km, altitude, keep_short_stops=False):
    """Return distance, moving minutes, and causal 200 m grade, or reject a ride."""
    if len(seconds) < 2 or not np.isfinite([seconds, km, altitude]).all():
        raise ValueError("missing_samples")
    dt, distance = np.diff(seconds), np.diff(km)
    if (dt <= 0).any() or (distance < 0).any():
        raise ValueError("nonmonotonic")
    if (dt > 30).any():
        raise ValueError("recording_gap")
    if (distance / dt * 3600 > 80).any():
        raise ValueError("speed_above_80")
    if np.ptp(altitude) == 0 or distance.sum() <= 0:
        raise ValueError("no_profile")
    end = np.arange(1, len(km))
    start = np.minimum(np.maximum(np.searchsorted(km, km[1:] - .2, side="right") - 1, 0), end-1)
    span = km[1:] - km[start]
    grade = np.divide(altitude[1:] - altitude[start], span*1000,
                      out=np.zeros(len(dt)), where=span > 0)
    moving = distance > 0
    if (np.abs(grade[moving]) > .5).any():
        raise ValueError("grade_above_50_percent")
    if keep_short_stops:
        # Diagnostic only: include internal zero-distance runs of at most 10 s.
        edges = np.flatnonzero(np.diff(np.r_[False, ~moving, False]))
        for a, b in zip(edges[::2], edges[1::2]):
            if a > 0 and b < len(dt) and dt[a:b].sum() <= 10:
                moving[a:b] = True
    return np.array([distance, dt/60*moving, grade])


def duplicate(a, b):
    """Conservative same-day dual-recording screen, independent of predictions."""
    if a["date"][:10] != b["date"][:10]:
        return False
    return (abs(a["distance_km"]-b["distance_km"]) <= .05*max(a["distance_km"], b["distance_km"])
            and abs(a["elapsed_minutes"]-b["elapsed_minutes"]) <= .05*max(a["elapsed_minutes"], b["elapsed_minutes"]))


def prepare(archive, output):
    if digest(archive) != SOURCE_SHA:
        raise ValueError("Unexpected source archive")
    output.mkdir(parents=True, exist_ok=True)
    if (output / "audit.json").exists():
        raise ValueError("Prepared data already exists; use a new output directory")
    rejected, candidates = Counter(), []
    with zipfile.ZipFile(archive) as z:
        records = json.loads(z.read(next(n for n in z.namelist() if n.endswith(".json"))))["RIDES"]
        for name in sorted(n for n in z.namelist() if n.endswith(".csv")):
            record = match_metadata(name, records)
            if record is None:
                rejected["ambiguous_metadata"] += 1
                continue
            if record["sport"] != "Bike":
                rejected["not_bike"] += 1
                continue
            if "G" not in record["data"] or "A" not in record["data"]:
                rejected["no_original_gps_or_altitude"] += 1
                continue
            data = np.genfromtxt(io.BytesIO(z.read(name)), delimiter=",", names=True)
            try:
                raw = intervals(data["secs"], data["km"], data["alt"])
                alternate = intervals(data["secs"], data["km"], data["alt"], True)
            except ValueError as e:
                rejected[str(e)] += 1
                continue
            if raw[1].sum() < 5 or raw[0].sum() < 1:
                rejected["below_5_minutes_or_1_km"] += 1
                continue
            stamp = datetime.strptime(record["date"], "%Y/%m/%d %H:%M:%S UTC").replace(tzinfo=timezone.utc).timestamp()
            candidates.append(dict(id=Path(name).stem, date=record["date"], start=stamp,
                end=stamp+float(data["secs"][-1]), distance_km=float(raw[0].sum()),
                moving_minutes=float(raw[1].sum()), elapsed_minutes=float(np.diff(data["secs"]).sum()/60),
                short_stop_minutes=float(alternate[1].sum()-raw[1].sum()),
                source_riding_minutes=float(record["METRICS"].get("time_riding", 0))/60,
                median_sample_seconds=float(np.median(np.diff(data["secs"]))), raw=raw,
                alternate_minutes=alternate[1]))
    # Prefer the finer recording in each potential duplicate pair.
    unique = []
    for ride in sorted(candidates, key=lambda r: (r["median_sample_seconds"], r["id"])):
        if any(duplicate(ride, other) for other in unique):
            rejected["possible_same_day_duplicate"] += 1
        else:
            unique.append(ride)
    kept, end = [], -float("inf")
    for ride in sorted(unique, key=lambda r: r["start"]):
        if ride["start"] < end:
            rejected["overlap"] += 1
            continue
        end = ride["end"]
        np.savez_compressed(output / (ride["id"]+".npz"), raw=ride.pop("raw"),
                            alternate_minutes=ride.pop("alternate_minutes"))
        kept.append(ride)
    audit = dict(source=SOURCE, source_sha256=SOURCE_SHA, athlete_count=1,
                 source_summaries=len(records), source_csvs=sum(n.endswith(".csv") for n in z.namelist()),
                 exclusions=dict(rejected), retained=len(kept),
                 hours={str(h): sum(r["moving_minutes"] >= h*60 for r in kept) for h in (2, 4, 6)},
                 distance_km=sum(r["distance_km"] for r in kept),
                 moving_minutes=sum(r["moving_minutes"] for r in kept),
                 short_stop_minutes=sum(r["short_stop_minutes"] for r in kept))
    (output / "rides.json").write_text(json.dumps(kept, indent=2)+"\n")
    (output / "audit.json").write_text(json.dumps(audit, indent=2)+"\n")
    return audit
