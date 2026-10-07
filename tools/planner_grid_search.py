"""Partition one planner search component into immutable grid databases."""

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys
import tempfile

from . import planner_grid as grid, planner_offline as offline, planner_runtime as runtime, step_request

ROOT = Path(__file__).resolve().parent.parent


def search_metadata(database, full=False):
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as db:
        metadata = {k: json.loads(v) for k, v in db.execute("SELECT key,value FROM metadata")}
        if metadata.get("schema") != 5:
            raise ValueError(f"Rebuild search package {database}: incompatible schema.")
        try:
            db.execute('SELECT rowid FROM addresses INDEXED BY address_cells LIMIT 0')
        except sqlite3.Error as error:
            raise ValueError(f"Rebuild search package {database}: missing address index.") from error
        if full and db.execute("PRAGMA quick_check").fetchone() != ("ok",):
            raise ValueError("Search database failed verification")
    return metadata


def search_lookup(source, output):
    with closing(sqlite3.connect(output, uri=True)) as db:
        db.execute("ATTACH DATABASE ? AS original", (source.resolve().as_uri() + "?mode=ro",))
        db.executescript("""PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
            CREATE VIRTUAL TABLE places USING rtree(id,west,east,south,north);
            INSERT INTO places SELECT id,MIN(lon,COALESCE(west,lon)),MAX(lon,COALESCE(east,lon)),
                MIN(lat,COALESCE(south,lat)),MAX(lat,COALESCE(north,lat)) FROM original.places;""")
        db.commit()


def search_shard(source, lookup, output, bounds, metadata):
    sys.path.insert(0, str(ROOT / "planner/search"))
    try:
        from storage import create, finish
    finally:
        sys.path.pop(0)
    db = create(output)
    try:
        db.execute("ATTACH DATABASE ? AS original", (source.resolve().as_uri() + "?mode=ro",))
        db.execute("ATTACH DATABASE ? AS lookup", (lookup.resolve().as_uri() + "?mode=ro",))
        db.execute("CREATE TEMP TABLE selected(id INTEGER PRIMARY KEY)")
        w, s, e, n = bounds
        db.execute("INSERT INTO selected SELECT id FROM lookup.places WHERE east>=? AND north>=? AND west<=? AND south<=?", bounds)
        left, right = int((w + 180) * 200), int((e + 180) * 200)
        for y in range(int((s + 90) * 200), int((n + 90) * 200) + 1):
            db.execute("""INSERT INTO addresses(rowid,street_id,house,lon,lat,source)
                SELECT rowid,street_id,house,lon,lat,source FROM original.addresses
                WHERE CAST((lat+90)*200 AS INTEGER)*72001+CAST((lon+180)*200 AS INTEGER) BETWEEN ? AND ?
                    AND lon>=? AND lat>=? AND lon<=? AND lat<=?""", [y * 72001 + left, y * 72001 + right, *bounds])
        db.execute("INSERT OR IGNORE INTO selected SELECT DISTINCT street_id FROM addresses")
        db.execute("INSERT INTO place_records SELECT * FROM original.place_records WHERE id IN selected ORDER BY id")
        db.execute("INSERT INTO place_contexts SELECT * FROM original.place_contexts WHERE id IN (SELECT context_id FROM place_records) ORDER BY id")
        db.commit()
        db.execute("DETACH DATABASE original"); db.execute("DETACH DATABASE lookup")
        finish(db, output, {**metadata, "bounds": bounds})
    except BaseException:
        db.close(); raise


def step(request):
    layer, = request["layers"].values()
    source, = layer.values()
    source = Path(source)
    output = Path(request["output"])
    metadata = search_metadata(source, full=True)
    component = request["options"]["component"]
    if metadata["component"] != component or metadata["bounds"] != request["options"]["bounds"]:
        raise ValueError("Search component or coverage differs")
    files, cells = {}, []
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        lookup = work / "lookup.sqlite"
        search_lookup(source, lookup)
        for name, bounds in grid.cells(metadata["bounds"]):
            shard = work / f"{name}.sqlite"
            search_shard(source, lookup, shard, bounds, metadata)
            logical = f"search/tiles/{component}/{name}.sqlite"
            files[logical] = offline.pack_file(shard, output / "objects")
            cells.append({"id": name, "bounds": bounds, "files": [logical]})
            shard.unlink()
    (output / "index.json").write_bytes(runtime.encoded({"format": 1, "kind": component,
        "metadata": metadata, "files": files, "cells": cells}))
    step_request.metrics(request, {"cells": len(cells)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
