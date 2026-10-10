"""Build rider-place tiles from the same POI database that serves search."""

import argparse
import gzip
import json
import math
import sqlite3
import sys
from contextlib import closing
from pathlib import Path

from . import planner_mvt as mvt, step_request

KINDS = Path(__file__).resolve().parents[1] / "planner/search/place-kinds.json"
# The only zoom: a route corridor reads few tiles, and rider places keep each tile small.
ZOOM = 11


def rider_kinds():
    """The rider-place kinds included in the archive."""
    return {kind for kinds in json.loads(KINDS.read_text()).values() for kind in kinds}


def pois(tile):
    """Each single point of the tile's `pois` layer as (id, properties, x, y, extent)."""
    for number, layer in mvt.fields(tile):
        if number != 3: continue
        name, extent, keys, values, features = None, 4096, [], [], []
        for field, value in mvt.fields(layer):
            if field == 1: name = value.decode()
            elif field == 2: features.append(value)
            elif field == 3: keys.append(value.decode())
            elif field == 4: values.append(next((v.decode() for f, v in mvt.fields(value) if f == 1), None))
            elif field == 5: extent = value
        if name != "pois": continue
        for feature in features:
            parts = dict(mvt.fields(feature))
            geometry = mvt.packed(parts.get(4, b""))
            # A single point is one MoveTo command with one zigzag-encoded coordinate pair.
            if parts.get(3) != 1 or len(geometry) != 3 or geometry[0] != 9: continue
            tags = mvt.packed(parts.get(2, b""))
            properties = {keys[k]: values[v] for k, v in zip(tags[::2], tags[1::2])}
            x, y = ((v >> 1) ^ -(v & 1) for v in geometry[1:])
            yield parts.get(1), properties, x, y, extent


def tile(places):
    """A vector tile with one `pois` layer of (id, properties, x, y) points."""
    layer = mvt.Layer("pois")
    for identity, properties, x, y in places:
        layer.add(identity, properties, mvt.POINT, [9, mvt.zigzag(x), mvt.zigzag(y)])
    return layer.encode()


def derive(database, destination):
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write

    kinds = rider_kinds()
    tiles = {}
    count = 0
    with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True)) as db:
        meta = {key: json.loads(value) for key, value in db.execute('SELECT key,value FROM metadata')}
        if meta.get('component') != 'pois':
            raise ValueError('Place tiles require the POI search component')
        bounds = meta['bounds']
        for source, kind, name, lon, lat in db.execute('SELECT source,kind,name,lon,lat FROM places ORDER BY source'):
            if kind not in kinds or not (bounds[0] <= lon <= bounds[2] and bounds[1] <= lat <= bounds[3]):
                continue
            if source[0] not in 'nwrQ' or not source[1:].isdigit() or not 0 < int(source[1:]) < 2**44:
                raise ValueError('A place has no representable source identity')
            identity = ('nwrQ'.index(source[0]) + 1) * 2**44 + int(source[1:])
            x = (lon + 180) / 360 * (1 << ZOOM)
            y = (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * (1 << ZOOM)
            properties = {'kind': kind, 'name': name, 'lon': str(lon), 'lat': str(lat)}
            tiles.setdefault((math.floor(x), math.floor(y)), []).append(
                (identity, properties, math.floor((x % 1) * mvt.EXTENT), math.floor((y % 1) * mvt.EXTENT)))
            count += 1
    # PMTiles needs a directory entry even when the region has no rider places.
    if not tiles:
        tiles[(int((bounds[0] + 180) / 360 * (1 << ZOOM)),
               int((1 - math.asinh(math.tan(math.radians(bounds[1]))) / math.pi) / 2 * (1 << ZOOM)))] = []
    with write(destination) as writer:
        for (x, y), points in sorted(tiles.items(), key=lambda item: zxy_to_tileid(ZOOM, *item[0])):
            writer.write_tile(zxy_to_tileid(ZOOM, x, y), gzip.compress(tile(points), mtime=0))
        writer.finalize({'tile_type': TileType.MVT, 'tile_compression': Compression.GZIP,
            'min_zoom': ZOOM, 'max_zoom': ZOOM, 'center_zoom': ZOOM,
            'min_lon_e7': round(bounds[0]*1e7), 'min_lat_e7': round(bounds[1]*1e7),
            'max_lon_e7': round(bounds[2]*1e7), 'max_lat_e7': round(bounds[3]*1e7),
            'center_lon_e7': round((bounds[0]+bounds[2])*5e6), 'center_lat_e7': round((bounds[1]+bounds[3])*5e6)}, {
            'name': 'OpenBikeComputer rider places', 'attribution': meta['attribution'], 'osm_sha256': meta['osm_sha256'],
            'vector_layers': [{'id': 'pois', 'minzoom': ZOOM, 'maxzoom': ZOOM,
                'fields': {'kind': 'String', 'name': 'String', 'lon': 'String', 'lat': 'String'}}]})
    return {'places': count, 'tiles': len(tiles)}


def step():
    """The `obc data` step `planner/places`: `places.pmtiles` from the database of `planner/search/pois`."""
    request = step_request.read()
    (database,) = request["layers"]["planner/search/pois"].values()
    step_request.metrics(request, derive(Path(database), Path(request["output"]) / "places.pmtiles"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(derive(args.database, args.output))


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
