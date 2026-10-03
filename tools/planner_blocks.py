"""Build the canonical planner grid from one verified regional release."""
import argparse
from collections import OrderedDict
from contextlib import closing
import gzip
import json
import math
from pathlib import Path, PurePosixPath
import sqlite3
import subprocess
import sys

try:
    from . import planner_cutout, planner_offline, planner_runtime, planner_maps, planner_mvt as mvt
except ImportError:
    import planner_cutout, planner_offline, planner_runtime, planner_maps, planner_mvt as mvt

ZOOM = 9
MAP_ZOOM = 11
# A copy of the basemap text fields: protomaps `layers(..., {lang: "en"})` and `planner-poi-icons`
# in `map-style.ts`. A style, `lang` or protomaps change must update it. `planner-network-labels`
# draws the overlay `ref` that `glyph_ranges` reads.
LABEL_KEYS ={"name", "name:en", "pgf:name", "name2", "pgf:name2", "name3", "pgf:name3",
              "ref", "ref:en", "shield_text", "addr_housenumber"}


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


def map_kinds(maps):
    """The tile archives of a map bundle; a data layer is present only when its recipe asks for it."""
    return ["basemap", "places", "overlays", "terrain"] + [layer for layer in planner_runtime.DATA_LAYERS if (maps / f"{layer}.pmtiles").exists()]


def map_tiles(source, output):
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import zxy_to_tileid
    from pmtiles.writer import Writer
    import struct
    import tempfile

    output.mkdir(parents=True, exist_ok=True)
    for kind in map_kinds(source):
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


def label_texts(tile):
    """Yield the distinct label strings of each layer of one vector tile."""
    for field, layer in mvt.fields(tile):
        if field != 3: continue
        keys, values, labels = [], [], set()
        for field, value in mvt.fields(layer):
            if field == 3: keys.append(value.decode())
            elif field == 4: values.append(next((text.decode() for number, text in mvt.fields(value) if number == 1), None))
            elif field == 2:
                for packed in (packed for number, packed in mvt.fields(value) if number == 2):
                    tags = mvt.packed(packed)
                    labels.update(zip(tags[::2], tags[1::2]))
        yield from {values[value] for key, value in labels if keys[key] in LABEL_KEYS and values[value]}


def glyph_ranges(source):
    """Return the indices (code point // 256) of the glyph ranges that labels and route references use."""
    from pmtiles.reader import MmapSource, all_tiles
    texts = set()
    with (source / "maps/basemap.pmtiles").open("rb") as file:
        for _, tile in all_tiles(MmapSource(file)):
            texts.update(label_texts(gzip.decompress(tile) if tile[:2] == b"\x1f\x8b" else tile))
    with closing(sqlite3.connect(f"{(source / 'routing/overlays.sqlite').resolve().as_uri()}?mode=ro", uri=True)) as db:
        texts.update(ref for (ref,) in db.execute("SELECT json_extract(properties, '$.ref') FROM attributes "
                                                  "UNION SELECT json_extract(properties, '$.ref') FROM routes") if ref)
    # MapLibre requests the glyphs of the drawn text: the style upper-cases some labels, and Arabic
    # letters become presentation forms U+FB50-U+FEFF.
    ranges = {ord(character) >> 8 for text in texts for character in text + text.upper()}
    return ranges | ({251, 252, 253, 254} if ranges & {6, 7, 8} else set())


def offline_fonts(source, release, work):
    """Join the used glyph ranges of each font stack into one file for offline selections.

    MapLibre keeps only the glyphs of the requested range, so every range path of a stack can
    share this file. A missing range file stalls the labels of each tile that requests it."""
    ranges, stacks = glyph_ranges(source), {}
    start = lambda name: int(PurePosixPath(name).stem.split("-")[0])
    for name in release["files"]:
        path = PurePosixPath(name)
        if path.parent.parent == PurePosixPath("maps/assets/fonts") and path.suffix == ".pbf":
            stacks.setdefault(path.parent.name, []).append(name)
    result = {}
    for stack, names in sorted(stacks.items()):
        target = work / "fonts" / f"{stack}.pbf"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(b"".join((source / name).read_bytes() for name in sorted(names, key=start)
                                    if start(name) >> 8 in ranges))
        result[target] = names
    return result


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
    shared = {name: add(source / name, name) for name in release["files"] if name.startswith("maps/assets/")}
    print("Joining offline fonts", flush=True)
    for path, names in offline_fonts(source, release, work).items():
        shared.update(dict.fromkeys(names, add(path, f"offline/fonts/{path.name}")))
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
        # Offline planners read places from the basemap and search, and overlays from the routing cells.
        # They have no data layers yet.
        if path.parent.name not in ("places", "overlays", *planner_runtime.DATA_LAYERS):
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
    for kind in map_kinds(source / "maps"):
        with (source / "maps" / f"{kind}.pmtiles").open("rb") as stream:
            reader = Reader(MmapSource(stream))
            header, info = reader.header(), reader.metadata()
        if kind == "overlays":
            # The grid packs the same routing graph under its own identity, which its route answers carry.
            if info.get("routing_package") != graph["source"]: raise ValueError("Overlay tiles use another routing package")
            info["routing_package"] = files["routing/blocks.json"]["sha256"]
        metadata(f"maps/{kind}.json", {**info, "tilejson": "3.0.0", "minzoom": header["min_zoom"],
            "maxzoom": header["max_zoom"], "bounds": release["terrain_bounds"] if kind == "terrain" else release["bounds"]})
    for name in release["files"]:
        if name.startswith(("search/model/", "device/")): add(source / name, name)
    catalog = {"format": 3, "source": identity, "release": {k:v for k,v in release.items() if k not in {"files", "source_files"}},
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
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        prepare(args.source, args.output)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner grid: {error}\n")


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
