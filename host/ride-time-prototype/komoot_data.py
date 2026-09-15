"""Private Komoot GPX adapter. Unknown recording time remains explicit."""

import argparse
from collections import Counter
from datetime import datetime
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET

import numpy as np

from long_data import digest

SOURCE = Path.home() / ".cache/openbikecomputer/ride-time/komoot"
OUTPUT = Path(".artifacts/ride-time-komoot")
POLICY = dict(max_active_gap_seconds=30, max_speed_kmh=80, max_abs_grade=.5,
              grade_window_m=200, min_minutes=5, min_km=1,
              duration_agreement_seconds=120, duration_agreement_fraction=.05,
              min_distance_coverage=.99)
SPORTS = {"mtb": "mtb", "racebike": "other", "touringbicycle": "other"}


def timestamp(value):
    date = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if date.tzinfo is None:
        raise ValueError("timestamp_without_timezone")
    return date.timestamp()


def read_gpx(path):
    points, segments = [], []
    for number, segment in enumerate(ET.parse(path).findall(".//{*}trkseg")):
        for p in segment.findall("{*}trkpt"):
            try:
                points.append((timestamp(p.find("{*}time").text), float(p.get("lat")),
                               float(p.get("lon")), float(p.find("{*}ele").text)))
                segments.append(number)
            except (AttributeError, TypeError, ValueError) as e:
                raise ValueError("invalid_point") from e
    a = np.asarray(points, dtype=float)
    if len(a) < 2 or not np.isfinite(a).all():
        raise ValueError("missing_or_nonfinite_points")
    if (np.abs(a[:, 1]) > 90).any() or (np.abs(a[:, 2]) > 180).any():
        raise ValueError("invalid_coordinates")
    if (np.diff(a[:, 0]) <= 0).any():
        raise ValueError("nonmonotonic_time")
    return a, np.asarray(segments)


def pause_overlap(seconds, events, start, duration):
    """Union explicit millisecond offsets; never subtract overlapping pauses twice."""
    ranges = []
    for event in events:
        a, b = float(event["start_ms"])/1000, float(event["end_ms"])/1000
        if not np.isfinite([a, b]).all() or a < 0 or b <= a or b > duration+1:
            raise ValueError("invalid_pause_event")
        ranges.append((start+a, start+b))
    merged = []
    for a, b in sorted(ranges):
        if merged and a <= merged[-1][1]:
            merged[-1][1] = max(b, merged[-1][1])
        else:
            merged.append([a, b])
    result = np.zeros(len(seconds)-1)
    for a, b in merged:
        result += np.maximum(0, np.minimum(seconds[1:], b)-np.maximum(seconds[:-1], a))
    return result


def distance_m(points):
    lat, lon = np.radians(points[:, 1]), np.radians(points[:, 2])
    a = np.sin(np.diff(lat)/2)**2 + np.cos(lat[:-1])*np.cos(lat[1:])*np.sin(np.diff(lon)/2)**2
    return 6371000*2*np.arcsin(np.sqrt(np.clip(a, 0, 1)))


def adapt(points, segments, metadata):
    """Return interval arrays and a motion-proxy audit, without fitting an estimator."""
    seconds, altitude = points[:, 0], points[:, 3]
    dt, distance = np.diff(seconds), distance_m(points)
    start, duration = timestamp(metadata["date"]), float(metadata["duration"])
    if not np.isfinite([start, duration, metadata["time_in_motion"]]).all():
        raise ValueError("invalid_summary")
    if not 0 <= metadata["time_in_motion"] <= duration+1:
        raise ValueError("invalid_summary")
    if abs(seconds[0]-start) > 60 or abs(seconds[-1]-seconds[0]-duration) > 60:
        raise ValueError("summary_time_mismatch")
    paused = pause_overlap(seconds, metadata.get("pause_events") or [], start, duration)
    active = np.maximum(0, dt-paused)
    segment_break = np.diff(segments) != 0
    unknown = (active > POLICY["max_active_gap_seconds"]) | segment_break
    # A pause can contain GPS drift. It supplies no evidence of riding pace.
    moving = (active > 0) & (distance > 0)
    speed = np.divide(distance*3.6, active, out=np.zeros(len(dt)), where=active > 0)
    speed_bad = moving & (speed > POLICY["max_speed_kmh"])
    unknown |= speed_bad
    # Reset the observed profile after any pause, stationary interval, or unknown gap.
    grade = np.zeros(len(dt))
    grade_bad = np.zeros(len(dt), dtype=bool)
    cumulative = np.r_[0., np.cumsum(distance)]
    block_start = 0
    for i in range(len(dt)):
        if unknown[i] or not moving[i]:
            block_start = i+1
            continue
        if paused[i] > 0:
            block_start = i
        target = cumulative[i+1]-POLICY["grade_window_m"]
        left = max(block_start, min(i, int(np.searchsorted(cumulative, target, side="right"))-1))
        span = cumulative[i+1]-cumulative[left]
        if span > 0:
            grade[i] = (altitude[i+1]-altitude[left])/span
            if abs(grade[i]) > POLICY["max_abs_grade"]:
                grade_bad[i] = True
                block_start = i+1
    usable = moving & ~unknown & ~grade_bad
    minutes = np.where(usable, active/60, 0.)
    # Unknown time is retained even if both endpoints have identical coordinates.
    unknown_minutes = np.where(unknown | grade_bad, active/60, 0.)
    raw = np.array([np.where(usable, distance/1000, 0.), minutes, grade])
    reset = unknown | grade_bad | ~moving | (paused > 0)
    after_reset = np.r_[True, reset[:-1]]
    learn = usable & ~after_reset & (paused == 0)
    observed = float(minutes.sum())
    tolerance = max(POLICY["duration_agreement_seconds"]/60,
                    POLICY["duration_agreement_fraction"]*metadata["time_in_motion"]/60)
    agrees = abs(observed-metadata["time_in_motion"]/60) <= tolerance
    coverage = float(distance[usable].sum()/distance.sum()) if distance.sum() > 0 else 0.
    proxy_eligible = bool(agrees and coverage >= POLICY["min_distance_coverage"]
                          and observed >= POLICY["min_minutes"]
                          and raw[0].sum() >= POLICY["min_km"])
    audit = dict(distance_km=float(raw[0].sum()), recorded_chord_km=float(distance.sum()/1000),
                 moving_minutes=observed, elapsed_minutes=float(dt.sum()/60),
                 source_moving_minutes=metadata["time_in_motion"]/60,
                 explicit_pause_minutes=float(paused.sum()/60),
                 zero_distance_active_minutes=float(active[(distance == 0) & ~unknown].sum()/60),
                 unknown_minutes=float(unknown_minutes.sum()), unknown_intervals=int(np.count_nonzero(unknown)),
                 unknown_chord_km=float(distance[unknown | grade_bad].sum()/1000),
                 speed_flags=int(speed_bad.sum()), grade_flags=int(grade_bad.sum()),
                 segment_breaks=int(segment_break.sum()), summary_agrees=agrees,
                 distance_coverage=coverage, proxy_eligible=proxy_eligible,
                 median_sample_seconds=float(np.median(dt)),
                 point_eligible=bool(not unknown.any() and not grade_bad.any() and agrees
                                     and observed >= POLICY["min_minutes"]
                                     and raw[0].sum() >= POLICY["min_km"]))
    return dict(raw=raw, elapsed_minutes=dt/60, unknown_minutes=unknown_minutes,
                recorded_distance_km=distance/1000, pause_minutes=paused/60,
                reset=reset, learn=learn), audit


def prepare(source, output):
    if output.exists() and any(output.iterdir()):
        raise ValueError("Output is not empty; preserve it and use a new directory")
    manifest_path = source / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    if not manifest.get("complete") or manifest.get("errors"):
        raise ValueError("Incomplete download manifest")
    files = {f["id"]: f for f in manifest["files"]}
    rides = manifest["rides"]
    if len(files) != len(manifest["files"]) or len({r["id"] for r in rides}) != len(rides):
        raise ValueError("Duplicate source IDs")
    if set(files) != {r["id"] for r in rides}:
        raise ValueError("Manifest ride/file mismatch")
    # Verify the complete snapshot before writing any prepared files.
    for id, f in files.items():
        if not id.isdigit() or f["path"] != f"downloads/{id}.gpx":
            raise ValueError("Unexpected source path")
        if digest(source / f["path"]) != f["sha256"]:
            raise ValueError("Source GPX hash mismatch: "+id)
    output.mkdir(parents=True, mode=0o700, exist_ok=True)
    candidates, rejected = [], []
    for r in rides:
        try:
            if not r.get("own") or r["sport"] not in SPORTS:
                raise ValueError("not_own_supported_cycling")
            p, segments = read_gpx(source / files[r["id"]]["path"])
            arrays, audit = adapt(p, segments, r)
            # Identity of timed geometry, independent of XML formatting and ride title.
            fingerprint = hashlib.sha256(p.astype("<f8").tobytes()).hexdigest()
            row = dict(id=r["id"], date=r["date"], start=float(p[0, 0]), end=float(p[-1, 0]),
                       bike=SPORTS[r["sport"]], source_sport=r["sport"],
                       fingerprint=fingerprint, **audit)
            candidates.append((row, arrays))
        except (ValueError, KeyError, ET.ParseError) as e:
            rejected.append(dict(id=r["id"], reason=str(e)))
    kept, seen = [], set()
    # Prefer finer samples for duplicate/overlapping recordings. Do not remove similar commutes.
    for row, arrays in sorted(candidates, key=lambda pair: (pair[0]["median_sample_seconds"], pair[0]["id"])):
        reason = "duplicate_timed_geometry" if row["fingerprint"] in seen else None
        if reason is None and any(row["start"] < k["end"] and row["end"] > k["start"] for k in kept):
            reason = "overlap"
        if reason:
            rejected.append(dict(id=row["id"], reason=reason))
            continue
        seen.add(row["fingerprint"])
        kept.append(row)
        np.savez_compressed(output / (row["id"]+".npz"), **arrays)
    kept.sort(key=lambda r: (r["start"], r["id"]))
    eligible = [r for r in kept if r["point_eligible"]]
    proxy = [r for r in kept if r["proxy_eligible"]]
    audit = dict(source_manifest_sha256=digest(manifest_path), source_rides=len(rides),
                 retained=len(kept), rejected=rejected, policy=POLICY,
                 point_eligible=len(eligible), proxy_eligible=len(proxy),
                 proxy_hours={str(h): sum(r["moving_minutes"] >= h*60 for r in proxy) for h in (2, 4, 6)},
                 eligible_hours={str(h): sum(r["moving_minutes"] >= h*60 for r in eligible) for h in (2, 4, 6)},
                 source_hours={str(h): sum(r["time_in_motion"] >= h*3600 for r in rides) for h in (2, 4, 6)},
                 flagged_rides=dict(unknown=sum(r["unknown_minutes"] > 0 for r in kept),
                                    summary_disagreement=sum(not r["summary_agrees"] for r in kept),
                                    speed=sum(r["speed_flags"] > 0 for r in kept),
                                    grade=sum(r["grade_flags"] > 0 for r in kept)),
                 bike_counts=dict(Counter(r["bike"] for r in kept)))
    for name, value in (("rides.json", kept), ("audit.json", audit)):
        (output / name).write_text(json.dumps(value, indent=2)+"\n")
    return audit


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    print(json.dumps(prepare(args.source, args.output), indent=2))
