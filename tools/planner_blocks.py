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


def map_tiles(source, output, kinds=None):
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import zxy_to_tileid
    from pmtiles.writer import Writer
    import struct
    import tempfile

    output.mkdir(parents=True, exist_ok=True)
    for kind in kinds or map_kinds(source):
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


def route_tiles(catalog, names):
    """Split a region route catalog into the documents of the grid cells `names`."""
    document = json.loads(catalog.read_bytes())
    if document["format"] != 1: raise ValueError("Unsupported route catalog")
    tiles = {name: [] for name in names}
    for record in sorted(document["routes"], key=lambda record: record["id"]):
        # A cell outside the grid has no file.
        for cell in record["cells"]:
            if cell in tiles: tiles[cell].append(record)
    return {name: {"format": 1, "routes": routes} for name, routes in tiles.items()}


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


def publish(source, routing, output, cache=None):
    from tools.planner_grid_components import publish as compose
    return compose(source, routing, output, cache)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-cache", type=Path, default=Path.home() / ".cache/obc/planner/sources")
    args = parser.parse_args()
    try:
        prepare(args.source, args.output, args.source_cache)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner grid: {error}\n")


def prepare(source, output, cache_root=None):
    """Build the canonical grid from one verified regional bake."""
    from tools.planner_components import Cache
    cache = Cache(cache_root) if cache_root else None
    publish(source, None, output, cache)
    identity, _ = planner_runtime.release(output, include_sources=False)
    print(f"Prepared grid release {identity}")


if __name__ == "__main__": main()
