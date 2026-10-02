"""Publish reusable routing, search and map objects before accepting area downloads."""
import argparse
from collections import OrderedDict
from contextlib import closing
import json
import math
from pathlib import Path
import sqlite3
import sys

from . import planner_cutout, planner_offline, planner_runtime, planner_maps

ZOOM = 9
MAP_ZOOM = 11


def tile(lon, lat, zoom=ZOOM):
    n = 1 << zoom
    return (min(n - 1, max(0, int((lon + 180) / 360 * n))),
            min(n - 1, max(0, int((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n))))


def box(z, x, y):
    n = 1 << z
    latitude = lambda row: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * row / n))))
    return [x / n * 360 - 180, latitude(y + 1), (x + 1) / n * 360 - 180, latitude(y)]


def intersects(a, b):
    return a[0] <= b[2] and a[2] >= b[0] and a[1] <= b[3] and a[3] >= b[1]


def cells(bounds):
    left, top = tile(bounds[0], bounds[3])
    right, bottom = tile(bounds[2], bounds[1])
    for x in range(left, right + 1):
        for y in range(top, bottom + 1):
            b = box(ZOOM, x, y)
            clipped = [max(b[0], bounds[0]), max(b[1], bounds[1]), min(b[2], bounds[2]), min(b[3], bounds[3])]
            if clipped[0] < clipped[2] and clipped[1] < clipped[3]:
                yield f"{ZOOM}-{x}-{y}", clipped


def map_tiles(source, output):
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import zxy_to_tileid
    from pmtiles.writer import Writer
    import struct
    import tempfile

    output.mkdir(parents=True, exist_ok=True)
    for kind in ("basemap", "places", "terrain"):
        target = output / kind
        target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".tiles-", dir=output) as temporary:
            cache = OrderedDict()
            try:
                with (source / f"{kind}.pmtiles").open("rb") as file:
                    read = MmapSource(file)
                    reader = Reader(read)
                    header, metadata = reader.header(), reader.metadata()
                    for (z, x, y), data in all_tiles(read):
                        level = min(z, MAP_ZOOM)
                        name = f"{level}-{x >> (z-level)}-{y >> (z-level)}"
                        stream = cache.pop(name, None)
                        if stream is None: stream = (Path(temporary) / name).open("ab")
                        stream.write(struct.pack("<QI", zxy_to_tileid(z, x, y), len(data)))
                        stream.write(data)
                        cache[name] = stream
                        if len(cache) > 16: cache.popitem(last=False)[1].close()
            finally:
                for stream in cache.values(): stream.close()
            for path in sorted(Path(temporary).iterdir()):
                with path.open("rb") as records, (target / f"{path.name}.pmtiles").open("wb") as destination:
                    writer = Writer(destination)
                    try:
                        while record := records.read(12):
                            tile_id, length = struct.unpack("<QI", record)
                            data = records.read(length)
                            if len(data) != length: raise ValueError("Incomplete tile staging file")
                            writer.write_tile(tile_id, data)
                        writer.finalize(dict(header), dict(metadata))
                    finally:
                        writer.tile_f.close()
                path.unlink()


def search_lookup(source, output):
    with closing(sqlite3.connect(output, uri=True)) as db:
        db.execute("ATTACH DATABASE ? AS original", (source.resolve().as_uri() + "?mode=ro",))
        db.executescript("""PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
            CREATE VIRTUAL TABLE places USING rtree(id,west,east,south,north);
            INSERT INTO places SELECT id,MIN(lon,COALESCE(west,lon)),MAX(lon,COALESCE(east,lon)),
                MIN(lat,COALESCE(south,lat)),MAX(lat,COALESCE(north,lat)) FROM original.places;""")
        db.commit()


def stage_sqlite(path, build):
    if path.exists(): return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".partial")
    temporary.unlink(missing_ok=True)
    try:
        build(temporary)
        with closing(sqlite3.connect(temporary)) as db:
            if db.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise ValueError("Grid database failed verification")
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def search_shard(source, lookup, output, bounds, metadata):
    sys.path.insert(0, str(planner_maps.ROOT / "apps/planner-search"))
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


def publish(source, routing, output):
    identity, release = planner_runtime.release(source, include_sources=False)
    if (output / "release.json").exists(): raise ValueError("Publication already exists")
    output.mkdir(parents=True, exist_ok=True)
    work = output / "building"
    work.mkdir(exist_ok=True)
    objects = output / "objects"
    files = {}
    def add(path, name):
        entry = planner_offline.pack_file(path, objects)
        files[name] = entry
        return name
    def metadata(name, value):
        path = work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(planner_runtime.encoded(value))
        return add(path, name)
    graph = json.loads((routing / "blocks.json").read_bytes())
    if graph["source"] != release["routing_package"]: raise ValueError("Routing blocks use another release")
    add(routing / "blocks.json", "routing/blocks.json")
    print("Publishing routing page packs", flush=True)
    for archive in graph["archives"]:
        for filename in ("pages.bin", "pages.idx"):
            add(routing / "packs" / archive / filename, f"routing/packs/{archive}/{filename}")
    routing_index = json.loads((routing / "catalog.json").read_bytes())
    routing_cells = {cell["id"]: cell for cell in routing_index["cells"]}
    for cell_id, cell in routing_cells.items():
        source_manifest = routing / cell["manifest"]
        name = add(source_manifest, f"offline/routing-cells/{cell_id}.json")
        cell["manifest"] = name.removeprefix("offline/")
        cell["sha256"] = files[name]["sha256"]
    shared = [add(source / name, name) for name in release["files"] if name.startswith("maps/assets/")]
    tiles = work / "tiles"
    if not (work / "tiles.complete").exists():
        if tiles.exists(): raise ValueError("Incomplete tile publication; use a fresh output directory")
        print("Partitioning map tiles", flush=True)
        map_tiles(source / "maps", tiles)
        (work / "tiles.complete").touch()
    map_blocks = []
    for path in sorted(tiles.glob("*/*.pmtiles")):
        z, x, y = map(int, path.stem.split("-"))
        name = add(path, f"maps/tiles/{path.parent.name}/{path.name}")
        # Offline planners read places from the basemap and search, so downloads carry no places packs.
        if path.parent.name != "places":
            map_blocks.append({"kind": path.parent.name, "tile": [z,x,y], "bounds": box(z, x, y), "files": [name]})
    search = source / "search" / f"{release['region']}.sqlite"
    lookup = work / "search-lookup.sqlite"
    stage_sqlite(lookup, lambda path: search_lookup(search, path))
    with closing(sqlite3.connect(search)) as db:
        search_metadata = {k: json.loads(v) for k, v in db.execute("SELECT * FROM metadata")}
    geographic = []
    all_cells = list(cells(release["bounds"]))
    for index, (name, bounds) in enumerate(all_cells):
        print(f"Publishing places and overlays {index + 1}/{len(all_cells)} · {name}", flush=True)
        places = work / "search" / f"{name}.sqlite"
        stage_sqlite(places, lambda path: search_shard(search, lookup, path, bounds, search_metadata))
        overlays = work / "overlays" / f"{name}.sqlite"
        stage_sqlite(overlays, lambda path: planner_cutout.overlays(source / "routing/overlays.sqlite", path, bounds, bounds, release["routing_package"]))
        geographic.append({"id": name, "bounds": bounds, "routing": routing_cells.get(name), "files": [add(places, f"search/tiles/{name}.sqlite"),
            add(overlays, f"routing/layers/{name}.sqlite")]})
    metadata(f"search/{release['region']}.grid.json", {"format": 2, "metadata": search_metadata,
        "cells": [{"id": name, "bounds": bounds} for name, bounds in all_cells]})
    metadata("routing/layers.json", [name for name, _ in all_cells])
    from pmtiles.reader import Reader, MmapSource
    for kind in ("basemap", "places", "terrain"):
        with (source / "maps" / f"{kind}.pmtiles").open("rb") as stream:
            reader = Reader(MmapSource(stream))
            header, info = reader.header(), reader.metadata()
        metadata(f"maps/{kind}.json", {**info, "tilejson": "3.0.0", "minzoom": header["min_zoom"],
            "maxzoom": header["max_zoom"], "bounds": release["terrain_bounds"] if kind == "terrain" else release["bounds"]})
    for name in release["files"]:
        if name.startswith(("search/model/", "device/")): add(source / name, name)
    catalog = {"format": 2, "source": identity, "release": {k:v for k,v in release.items() if k not in {"files", "source_files"}},
        "routing_source": graph["source"], "map_blocks": map_blocks, "cells": geographic,
        "shared": shared, "files": dict(files), "zoom": ZOOM, "map_zoom": MAP_ZOOM}
    metadata("offline/catalog.json", catalog)
    document = {**catalog["release"], "routing_package": files["routing/blocks.json"]["sha256"],
        "source_files": {}, "grid": {"format": 2, "zoom": ZOOM, "map_zoom": MAP_ZOOM}, "files": files}
    planner_offline.atomic_write(output / "release.json", planner_runtime.encoded(document))
    return catalog


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("routing", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    publish(args.source, args.routing, args.output)


def prepare(source, output):
    """Build the canonical grid from one verified regional bake."""
    _, document = planner_runtime.release(source, include_sources=False)
    output.mkdir(parents=True, exist_ok=True)
    if (output / "release.json").exists(): raise ValueError("Publication already exists")
    work = output / "building"
    work.mkdir(exist_ok=True)
    selection = work / "cells.json"
    selection.write_bytes(planner_runtime.encoded([{"id": name, "bounds": bounds} for name, bounds in cells(document["bounds"])]))
    routing = work / "routing"
    if not (routing / "catalog.json").exists():
        planner_maps.run("cargo", "build", "--locked", "--release", "-p", "route-build", "--bin", "route-blocks", cwd=planner_maps.ROOT)
        planner_maps.run(planner_maps.ROOT / "target/release/route-blocks", source / "routing", routing, "--cells", selection)
    publish(source, routing, output)
    identity, _ = planner_runtime.release(output, include_sources=False)
    print(f"Prepared grid release {identity}")


if __name__ == "__main__": main()
