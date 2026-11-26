"""Derive the steepness and trail grade tiles of the ways from `ways.sqlite` of the routing bake."""

from contextlib import closing
import gzip
from pathlib import Path
import sqlite3

from . import planner_mvt as mvt, step_request
from .planner_overlays import MAX_ZOOM, coordinates, merge, tiles_of

MIN_ZOOM = 12
# The zoom of a block of `ways.sqlite`: a block is the tile `x * 512 + y` of this zoom.
BLOCK_ZOOM = 9


def derive(database, destination, bounds, credit):
    """Write `ways.pmtiles` one block at a time: a tile reads only the rows of its block. `credit` is the
    attribution of the OSM data."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write

    largest = {}
    destination.parent.mkdir(parents=True, exist_ok=True)
    # An interrupted bake leaves no archive that looks complete.
    partial = destination.with_name(destination.name + ".partial")
    with closing(sqlite3.connect(f"{database.resolve().as_uri()}?mode=ro", uri=True)) as db, write(partial) as writer:
        if db.execute("PRAGMA user_version").fetchone()[0] != 1: raise ValueError("ways.sqlite is not format 1")
        # In the order of their tile IDs, the tiles of each deeper zoom come block by block too.
        blocks = sorted((block for (block,) in db.execute("SELECT DISTINCT block FROM stretches")),
                        key=lambda block: zxy_to_tileid(BLOCK_ZOOM, *divmod(block, 512)))
        for zoom in range(MIN_ZOOM, MAX_ZOOM + 1):
            shift, sizes = zoom - BLOCK_ZOOM, [0]
            for block in blocks:
                groups = {}
                for way, slope, mtb, sac, trail, blob in db.execute(
                        "SELECT way, slope, mtb, sac, trail, coordinates FROM stretches WHERE block=? AND minzoom<=? ORDER BY rowid",
                        (block, zoom)):
                    tags = {"slope": slope, "mtb": mtb, "sac": sac, "trail": trail == 1 or None}
                    groups.setdefault(tuple((k, v) for k, v in tags.items() if v is not None), []).append((way, coordinates(blob)))
                layers = {}
                for tags, lines in groups.items():
                    for identity, points in merge(lines):
                        for (x, y), runs in tiles_of(points, zoom).items():
                            if (x >> shift, y >> shift) == divmod(block, 512):
                                layers.setdefault((x, y), mvt.Layer("ways")).add(identity, dict(tags), mvt.LINESTRING, mvt.line(runs))
                for tile, key in sorted((zxy_to_tileid(zoom, *key), key) for key in layers):
                    data = gzip.compress(layers[key].encode(), mtime=0)
                    writer.write_tile(tile, data)
                    sizes.append(len(data))
            largest[zoom] = max(sizes)
            print(f"Ways zoom {zoom}: {len(sizes) - 1} tiles", flush=True)
        west, south, east, north = bounds
        e7 = lambda value: round(value * 1e7)
        writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                         "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                         "center_zoom": MIN_ZOOM, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2)}, {
            "name": "OpenBikeComputer steepness and trail grades",
            "attribution": f'<a href="https://www.openstreetmap.org/copyright">Ways {credit}</a>',
            "ways_format": 1,
            "vector_layers": [{"id": "ways", "minzoom": MIN_ZOOM, "maxzoom": MAX_ZOOM}]})
        tiles, size = writer.addressed_tiles, writer.offset
    partial.replace(destination)
    return {"tiles": tiles, "bytes": size, "largest_bytes": largest}


def step():
    """The `obc data` step `planner/ways`: `ways.pmtiles` from `ways.sqlite` of `planner/routing-ways`."""
    request = step_request.read()
    database = Path(request["layers"]["planner/routing-ways"]["ways/ways.sqlite"])
    output = Path(request["output"]) / "ways.pmtiles"
    options = request["options"]
    step_request.metrics(request, derive(database, output, options["bounds"], options["attribution"]))


if __name__ == "__main__":
    step()
