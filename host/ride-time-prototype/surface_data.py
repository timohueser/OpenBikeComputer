"""Private regional cohort and past-only OSM observation features."""

import argparse
from collections import Counter
import gzip
import json
from pathlib import Path
import sqlite3
import zlib

import numpy as np

from data import CACHE, parse_record, read_rides, split_for
from enrichment_data import load_track
from enrichment_match import interval_status, project
from matching_v2 import CONFIG, PathNetwork, common_composition

OUTPUT = Path(".artifacts/ride-time-surface")
TRACKS = CACHE / "surface-tracks.sqlite"
TAGS = ("highway", "surface")


def extract():
    if TRACKS.exists():
        raise SystemExit(f"Refusing to replace {TRACKS}")
    eligible = {ride for _, ride, *_ in read_rides(CACHE / "fitrec.sqlite")}
    with sqlite3.connect(TRACKS) as db, gzip.open(CACHE / "endomondoHR.json.gz", "rt") as stream:
        db.execute("CREATE TABLE tracks (ride INTEGER PRIMARY KEY, user INTEGER, sport TEXT, start REAL, "
                   "west REAL, south REAL, east REAL, north REAL, data BLOB)")
        for index, line in enumerate(stream):
            row = parse_record(line)
            if row.get("id") in eligible:
                values = np.array([row[k] for k in ("longitude", "latitude", "timestamp")], dtype="<f8")
                lon, lat, stamp = values
                if lon.min() >= 9 and lon.max() <= 10 and lat.min() >= 56 and lat.max() <= 57:
                    db.execute("INSERT INTO tracks VALUES (?,?,?,?,?,?,?,?,?)", (
                        row["id"], row["userId"], row["sport"], float(stamp[0]),
                        float(lon.min()), float(lat.min()), float(lon.max()), float(lat.max()),
                        zlib.compress(values.tobytes(), 1)))
            if (index + 1) % 25000 == 0:
                db.commit()
                print(f"Read {index+1:,} records", flush=True)
    selection = cohort()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / "cohort.json").write_text(json.dumps(selection, indent=2) + "\n")
    print(json.dumps(counts(selection), indent=2), flush=True)


def cohort():
    with sqlite3.connect(TRACKS) as db:
        return [dict(ride=r, user=u, sport=s, start=t, split=split_for(u))
                for r, u, s, t in db.execute("SELECT ride,user,sport,start FROM tracks ORDER BY user,start,ride")]


def counts(rows):
    return {split: dict(rides=len(group), riders=len({r["user"] for r in group}),
                        mtb_rides=sum(r["sport"] == "mountain bike" for r in group),
                        mtb_riders=len({r["user"] for r in group if r["sport"] == "mountain bike"}))
            for split in ("development", "calibration", "test")
            for group in [[r for r in rows if r["split"] == split]]}


def causal_compositions(graph, track):
    """Forward filtering: interval i depends only on points 0..i+1.

    Normalize each forward row to prevent cost growth. Reset at gaps or disconnected
    candidate sets. All retained transitions contribute to conservative tag fractions.
    Quality checks use geometry and timestamp spacing, never measured cycling speed.
    """
    xy = project(track[:, :2], graph.origin)
    previous, costs = [], None
    result = []
    for i, point in enumerate(xy):
        current = graph.candidates(point)
        emission = np.array([c["offset"]**2 / (2*CONFIG["gps_sigma_m"]**2) for c in current])
        tags = {}
        next_cost = emission
        if i and previous and current:
            seconds = track[i, 2] - track[i-1, 2]
            chord = float(np.linalg.norm(point - xy[i-1]))
            if 0 < seconds <= CONFIG["gap_seconds"] and chord <= 750:
                lengths = np.array([[graph.route(a, b) for b in current] for a in previous])
                pairs = costs[:, None] + np.abs(lengths-chord)/CONFIG["transition_scale_m"] + emission[None, :]
                best = float(np.min(pairs))
                if np.isfinite(best):
                    next_cost = np.min(pairs, axis=0) - best
                    ai, bi = np.unravel_index(np.argmin(pairs), pairs.shape)
                    status = interval_status(previous[ai], current[bi], float("inf"), float("inf"),
                                             True, True, chord, float(lengths[ai, bi]))
                    if status == "accepted":
                        paths = [graph.route_pieces(previous[a], current[b])[1]
                                 for a, b in np.argwhere(pairs-best <= CONFIG["accepted_margin"]+1e-9)]
                        tags = {tag: common_composition([graph.composition(p, tag) for p in paths]) for tag in TAGS}
        if i:
            result.append(tags)
        previous, costs = current, next_cost
    return result


def offline_compositions(graph, track):
    xy, candidates, chosen, alternatives = graph.analyse(track)
    result = []
    for i, options in enumerate(alternatives):
        a = candidates[i][chosen[i]] if chosen[i] >= 0 else None
        b = candidates[i+1][chosen[i+1]] if chosen[i+1] >= 0 else None
        chord = float(np.linalg.norm(xy[i+1]-xy[i]))
        length = graph.route(a, b) if a and b else float("inf")
        status = interval_status(a, b, float("inf"), float("inf"), bool(options), True, chord, length)
        result.append({tag: common_composition([graph.composition(p, tag) for _, p in options]) for tag in TAGS}
                      if status == "accepted" else {})
    return result


def match():
    with gzip.open(".artifacts/ride-time-enrichment/ways.json.gz", "rt") as f:
        graph = PathNetwork(json.load(f), [9.5, 56.5])
    rows = cohort()
    destination = OUTPUT / "matches"
    destination.mkdir(parents=True, exist_ok=True)
    for i, row in enumerate(rows):
        path = destination / f"{row['ride']}.json.gz"
        if path.exists():
            continue
        track = load_track(TRACKS, row["ride"])
        result = dict(offline=offline_compositions(graph, track), causal=causal_compositions(graph, track))
        with gzip.open(path, "wt") as f:
            json.dump(result, f, separators=(",", ":"))
        if (i+1) % 20 == 0:
            print(f"Matched {i+1}/{len(rows)} rides", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("extract", "match"))
    args = parser.parse_args()
    extract() if args.stage == "extract" else match()
