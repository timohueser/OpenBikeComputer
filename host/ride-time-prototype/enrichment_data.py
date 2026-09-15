"""Private development-track cache and deterministic geographic pilot selection."""

import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3
import zlib

import numpy as np

from data import CACHE, parse_record, read_rides


def extract(output):
    if output.exists():
        raise SystemExit(f"Refusing to replace {output}")
    eligible = {ride for _, ride, *_ in read_rides(CACHE / "fitrec.sqlite", "development")}
    with sqlite3.connect(output) as db, gzip.open(CACHE / "endomondoHR.json.gz", "rt") as stream:
        db.execute("CREATE TABLE tracks (ride INTEGER PRIMARY KEY, user INTEGER, sport TEXT, start REAL, "
                   "west REAL, south REAL, east REAL, north REAL, data BLOB)")
        stored = 0
        for i, line in enumerate(stream):
            row = parse_record(line)
            if row.get("id") not in eligible:
                continue
            values = np.array([row[k] for k in ("longitude", "latitude", "timestamp")], dtype="<f8")
            lon, lat, stamp = values
            db.execute("INSERT INTO tracks VALUES (?,?,?,?,?,?,?,?,?)", (
                int(row["id"]), int(row["userId"]), row["sport"], float(stamp[0]),
                float(lon.min()), float(lat.min()), float(lon.max()), float(lat.max()),
                zlib.compress(values.tobytes(), 1)))
            stored += 1
            if stored % 3000 == 0:
                db.commit()
                print(f"Cached {stored} development tracks", flush=True)
    print(f"Stored {stored} development tracks", flush=True)


def survey(database):
    with sqlite3.connect(database) as db:
        rows = db.execute("SELECT user,sport,west,south,east,north FROM tracks").fetchall()
    cells = {}
    for user, sport, west, south, east, north in rows:
        if east-west > 1 or north-south > 1:
            continue
        key = (int(np.floor((west+east)/2)), int(np.floor((south+north)/2)))
        group = cells.setdefault(key, {"rides": 0, "mtb": 0, "users": set(), "mtb_users": set()})
        group["rides"] += 1
        group["users"].add(user)
        if sport == "mountain bike":
            group["mtb"] += 1
            group["mtb_users"].add(user)
    top = sorted(cells.items(), key=lambda x: (len(x[1]["mtb_users"]), len(x[1]["users"])), reverse=True)[:25]
    print(json.dumps([dict(cell=k, **{a: len(b) if isinstance(b, set) else b for a,b in v.items()})
                      for k,v in top], indent=2))


def select(database, bbox, output, count=80):
    """Balance activity labels and riders without inspecting matches or forecast errors."""
    west, south, east, north = bbox
    with sqlite3.connect(database) as db:
        rows = db.execute("SELECT ride,user,sport,start FROM tracks WHERE west>=? AND south>=? AND east<=? AND north<=?",
                          (west, south, east, north)).fetchall()
    selected, used = [], Counter()
    pools = [[r for r in rows if (r[2] == "mountain bike") == mtb] for mtb in (True, False)]
    for pool in pools:
        pool.sort(key=lambda r: hashlib.sha256(f"obc-enrichment-v1:{r[0]}".encode()).hexdigest())
        for _ in range(count//2):
            if not pool:
                break
            least = min(used[r[1]] for r in pool)
            i = next(i for i,r in enumerate(pool) if used[r[1]] == least)
            row = pool.pop(i)
            selected.append(dict(ride=row[0], user=row[1], sport=row[2], start=row[3]))
            used[row[1]] += 1
    manifest = dict(bbox=bbox, eligible=len(rows), eligible_mtb=sum(r[2]=="mountain bike" for r in rows),
                    eligible_users=len({r[1] for r in rows}), selection="80 requested; 40 MTB + 40 other; rider-balanced stable hash order",
                    tracks=selected)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(manifest, indent=2)+"\n")
    print(json.dumps({k:v for k,v in manifest.items() if k != "tracks"}, indent=2))
    print(f"Selected {len(selected)} rides from {len(used)} riders")


def load_track(database, ride):
    with sqlite3.connect(database) as db:
        blob, = db.execute("SELECT data FROM tracks WHERE ride=?", (ride,)).fetchone()
    return np.frombuffer(zlib.decompress(blob), dtype="<f8").reshape(3, -1).T.copy()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("extract", "survey", "select"))
    parser.add_argument("--database", type=Path, default=CACHE / "development-tracks.sqlite")
    parser.add_argument("--bbox", nargs=4, type=float)
    parser.add_argument("--output", type=Path, default=Path(".artifacts/ride-time-enrichment/selection.json"))
    args = parser.parse_args()
    if args.stage == "extract":
        extract(args.database)
    elif args.stage == "survey":
        survey(args.database)
    else:
        if args.bbox is None:
            parser.error("select requires --bbox west south east north")
        select(args.database, args.bbox, args.output)
