"""Prepare a private FitRec cache. No downloaded ride data belongs in Git."""

import argparse
import ast
from collections import Counter
import gzip
import hashlib
import json
import math
from pathlib import Path
import sqlite3
import time
import zlib

import numpy as np

URL = "https://mcauleylab.ucsd.edu/public_datasets/gdrive/fitrec/endomondoHR.json.gz"
CYCLING = {"bike", "bike (transport)", "mountain bike"}
CACHE = Path.home() / ".cache/openbikecomputer/ride-time"


def parse_record(line):
    """Accept JSON or Python literals without executing the input."""
    if '"' not in line and "\\" not in line:
        try:
            return json.loads(line.replace("'", '"'))
        except json.JSONDecodeError:
            pass
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return ast.literal_eval(line)


def split_for(user):
    value = int.from_bytes(hashlib.sha256(f"obc-fitrec-v1:{user}".encode()).digest()[:4], "big") % 100
    return "development" if value < 60 else "calibration" if value < 80 else "test"


def intervals(row):
    """Return distance km, elapsed minutes, trailing grade, and quality flags."""
    fields = [np.asarray(row[k], dtype=np.float64) for k in ("latitude", "longitude", "altitude", "timestamp")]
    if len({len(x) for x in fields}) != 1 or len(fields[0]) < 3:
        raise ValueError("array_lengths")
    lat, lon, altitude, stamp = fields
    if not all(np.isfinite(x).all() for x in fields):
        raise ValueError("nonfinite_record")
    if np.any(np.abs(lat) > 90) or np.any(np.abs(lon) > 180):
        raise ValueError("coordinates")
    lat, lon = np.radians(lat), np.radians(lon)
    a = np.sin(np.diff(lat) / 2) ** 2 + np.cos(lat[:-1]) * np.cos(lat[1:]) * np.sin(np.diff(lon) / 2) ** 2
    distance = 6371.0088 * 2 * np.arcsin(np.sqrt(np.clip(a, 0, 1)))
    elapsed = np.diff(stamp) / 60
    cumulative = np.r_[0.0, np.cumsum(distance)]
    start = np.maximum(0, np.searchsorted(cumulative, cumulative[1:] - 0.2, side="right") - 1)
    support = cumulative[1:] - cumulative[start]
    grade = np.divide(altitude[1:] - altitude[start], 1000 * support,
                      out=np.zeros_like(distance), where=support > 0)
    speed = np.divide(60 * distance, elapsed, out=np.zeros_like(distance), where=elapsed > 0)
    flags = ((elapsed <= 0).astype(np.uint8)
             | ((distance < 0.003).astype(np.uint8) << 1)
             | (((speed < 1) | (speed > 80)).astype(np.uint8) << 2)
             | ((np.abs(grade) > 0.5).astype(np.uint8) << 3))
    return np.array([distance, elapsed, grade, flags], dtype="<f4")


def valid_mask(values, cap_seconds=30):
    return (values[3] == 0) & (values[1] <= cap_seconds / 60)


def read_rides(database, split=None, max_rides=60, audit=None):
    with sqlite3.connect(database) as db:
        query = "SELECT user, ride, start, sport, points, data FROM rides"
        args = ()
        if split:
            query += " WHERE split = ?"
            args = (split,)
        query += " ORDER BY user, start, ride"
        counts = Counter()
        latest_end = {}
        audit = audit if audit is not None else Counter()
        for user, ride, start, sport, points, blob in db.execute(query, args):
            counts[user] += 1
            if max_rides and counts[user] > max_rides:
                continue
            audit["candidate_records"] += 1
            values = np.frombuffer(zlib.decompress(blob), dtype="<f4").reshape(4, points - 1).copy()
            if start < latest_end.get(user, -math.inf) - 1.0:
                audit["overlapping_records_excluded"] += 1
                continue
            end = start + float(np.cumsum(values[1].astype(float)).max()) * 60
            if end <= start:
                audit["nonpositive_duration_records_excluded"] += 1
                continue
            latest_end[user] = end
            yield user, ride, start, sport, values


def prepare(source, output):
    if output.exists():
        raise SystemExit(f"Refusing to replace {output}; use a different output path.")
    output.parent.mkdir(parents=True, exist_ok=True)
    counts, sports, excluded, flags = Counter(), Counter(), Counter(), Counter()
    users = {name: set() for name in ("development", "calibration", "test")}
    cycling_users = set()
    spacing = np.zeros(3602, dtype=np.int64)
    point_counts = Counter()
    clock = time.perf_counter()
    with source.open("rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    with sqlite3.connect(output) as db, gzip.open(source, "rt") as stream:
        db.execute("CREATE TABLE rides (user INTEGER, ride INTEGER PRIMARY KEY, start INTEGER, "
                   "sport TEXT, split TEXT, points INTEGER, data BLOB)")
        for line in stream:
            counts["all_records"] += 1
            try:
                row = parse_record(line)
                sport = row.get("sport", "missing")
                sports[sport] += 1
                if sport not in CYCLING:
                    continue
                counts["cycling_records"] += 1
                cycling_users.add(int(row["userId"]))
                values = intervals(row)
                point_counts[str(values.shape[1] + 1)] += 1
                user, ride = int(row["userId"]), int(row["id"])
                split = split_for(user)
                blob = zlib.compress(values.tobytes(), level=1)
                db.execute("INSERT INTO rides VALUES (?, ?, ?, ?, ?, ?, ?)",
                           (user, ride, int(row["timestamp"][0]), sport, split, values.shape[1] + 1, blob))
                users[split].add(user)
                counts["stored_records"] += 1
                counts[f"{split}_records"] += 1
                counts["intervals"] += values.shape[1]
                counts["records_with_speed"] += int("speed" in row)
                seconds = np.rint(values[1].astype(float) * 60)
                spacing += np.bincount(np.clip(seconds + 1, 0, 3601).astype(int), minlength=3602)
                for label, mask in (("nonpositive_dt", values[1] <= 0),
                                    ("dt_above_30s", values[1] > 0.5),
                                    ("displacement_below_3m", (values[3].astype(int) & 2) != 0),
                                    ("speed_outside_1_80", (values[3].astype(int) & 4) != 0),
                                    ("grade_above_50pct", (values[3].astype(int) & 8) != 0),
                                    ("grade_outside_anchors", np.abs(values[2]) > 0.2)):
                    flags[label] += int(mask.sum())
                for cap in (15, 30):
                    valid = valid_mask(values, cap)
                    counts[f"accepted_intervals_{cap}s"] += int(valid.sum())
                    counts[f"accepted_km_{cap}s"] += float(values[0, valid].sum())
                    counts[f"accepted_minutes_{cap}s"] += float(values[1, valid].sum())
            except (ValueError, KeyError, TypeError, SyntaxError, sqlite3.IntegrityError) as exc:
                excluded[str(exc)[:80]] += 1
            if counts["all_records"] % 25000 == 0:
                db.commit()
                print(f"Read {counts['all_records']:,} records; stored {counts['stored_records']:,} cycling records", flush=True)
        db.execute("CREATE INDEX rider_order ON rides(user, start, ride)")
        db.execute("CREATE INDEX split_order ON rides(split, user, start, ride)")
    total = spacing.sum()
    quantiles = {str(q): int(np.searchsorted(np.cumsum(spacing), q * total) - 1) for q in (0.5, 0.9, 0.95, 0.99)}
    audit = dict(source_url=URL, source_sha256=digest, source_bytes=source.stat().st_size,
                 citation="Ni, Muhlstein and McAuley (WWW 2019), Modeling heart rate and activity data for personalized fitness recommendation",
                 terms_url="https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html",
                 terms="Academic use only; no redistribution or commercial use, per source page.",
                 counts=dict(counts), sports=dict(sports), exclusions=dict(excluded), flags=dict(flags),
                 cycling_users=len(cycling_users), users={k: len(v) for k, v in users.items()},
                 point_counts=dict(point_counts), rounded_spacing_quantiles_seconds=quantiles,
                 preparation_seconds=round(time.perf_counter() - clock, 2))
    output.with_suffix(".audit.json").write_text(json.dumps(audit, indent=2) + "\n")
    print(json.dumps(audit, indent=2), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=CACHE / "endomondoHR.json.gz")
    parser.add_argument("--output", type=Path, default=CACHE / "fitrec.sqlite")
    args = parser.parse_args()
    prepare(args.source, args.output)
