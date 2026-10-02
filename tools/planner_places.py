"""Derive the rider places archive from the basemap's most detailed `pois` layer."""

import argparse
import gzip
import json
import math
from pathlib import Path

from . import planner_mvt as mvt

KINDS = Path(__file__).resolve().parents[1] / "builder/app/src/lib/planner/poi-kinds.json"
# The only zoom: a route corridor reads few tiles, and rider places keep each tile small.
ZOOM = 11


def rider_kinds():
    """The `pois` kinds the planner shows: the keys of every category's `kinds` in the web planner."""
    return {kind for category in json.loads(KINDS.read_text()).values() for kind in category["kinds"]}


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


def derive(basemap, destination):
    """Write the rider places of the basemap's deepest zoom, one copy each, into tiles at `ZOOM`."""
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write

    kinds = rider_kinds()
    with basemap.open("rb") as stream:
        source = MmapSource(stream)
        reader = Reader(source)
        header, metadata = reader.header(), reader.metadata()
        if header["tile_type"] != TileType.MVT or header["tile_compression"] not in (Compression.NONE, Compression.GZIP):
            raise ValueError("The basemap must hold MVT tiles")
        places = {}
        for (z, tx, ty), data in all_tiles(source):
            if z != header["max_zoom"]: continue
            if header["tile_compression"] == Compression.GZIP: data = gzip.decompress(data)
            for identity, properties, x, y, extent in pois(data):
                if properties.get("kind") not in kinds: continue
                if identity is None: raise ValueError("A basemap place has no feature ID")
                # Tiles repeat points near their edges; the copy inside its own tile is exact.
                inside = 0 <= x < extent and 0 <= y < extent
                if inside or identity not in places:
                    places[identity] = ({k: properties[k] for k in ("kind", "name", "name:en") if properties.get(k) is not None},
                                        (tx + x / extent) / (1 << z), (ty + y / extent) / (1 << z))
    tiles = {}
    for identity, (properties, u, v) in sorted(places.items()):
        x, y = u * (1 << ZOOM), v * (1 << ZOOM)
        tiles.setdefault((math.floor(x), math.floor(y)), []).append(
            (identity, properties, math.floor((x % 1) * mvt.EXTENT), math.floor((y % 1) * mvt.EXTENT)))
    if not tiles: raise ValueError("The basemap has no rider places")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with write(destination) as writer:
        for (x, y), points in sorted(tiles.items(), key=lambda item: zxy_to_tileid(ZOOM, *item[0])):
            writer.write_tile(zxy_to_tileid(ZOOM, x, y), gzip.compress(tile(points), mtime=0))
        writer.finalize({**header, "tile_compression": Compression.GZIP, "center_zoom": ZOOM}, {
            "name": "OpenBikeComputer rider places", "attribution": metadata.get("attribution", ""),
            "vector_layers": [{"id": "pois", "minzoom": ZOOM, "maxzoom": ZOOM,
                               "fields": {"kind": "String", "name": "String", "name:en": "String"}}]})
    return {"places": len(places), "tiles": len(tiles)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("basemap", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(derive(args.basemap, args.output))


if __name__ == "__main__":
    main()
