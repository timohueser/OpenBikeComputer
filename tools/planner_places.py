"""Derive the rider places archive from the basemap's most detailed `pois` layer."""

import argparse
import gzip
import math
from pathlib import Path
import re

KINDS = Path(__file__).resolve().parents[1] / "builder/app/src/lib/planner/poi-kinds.ts"
# The only zoom: a route corridor reads few tiles, and rider places keep each tile small.
ZOOM = 11
EXTENT = 4096


def rider_kinds():
    """The `pois` kinds the planner shows: the keys of every category's `kinds` in the web planner."""
    text = KINDS.read_text()
    blocks = re.findall(r"\bkinds\s*:\s*\{([^{}]*)\}", text)
    entry = re.compile(r"""(?:(\w+)|'(\w+)'|"(\w+)")\s*:\s*(?:'[^'\\]*'|"[^"\\]*")""")
    # Every `kinds` object must parse completely, or the archive would silently miss places.
    if not blocks or len(blocks) != len(re.findall(r"\bkinds\s*:", text)) or any(
            re.sub(r"[\s,]", "", entry.sub("", block)) for block in blocks):
        raise ValueError(f"Cannot read every rider place kind from {KINDS.name}")
    return {"".join(match) for block in blocks for match in entry.findall(block)}


def read_varint(data, i):
    value = shift = 0
    while True:
        value |= (data[i] & 0x7F) << shift
        i += 1
        if data[i - 1] < 0x80: return value, i
        shift += 7


def fields(data):
    i = 0
    while i < len(data):
        key, i = read_varint(data, i)
        if key & 7 == 0: value, i = read_varint(data, i)
        elif key & 7 == 2:
            length, i = read_varint(data, i)
            value, i = data[i:i + length], i + length
        elif key & 7 in (1, 5): value, i = None, i + (8 if key & 7 == 1 else 4)
        else: raise ValueError("Unsupported protobuf wire type")
        yield key >> 3, value


def packed(data):
    i, values = 0, []
    while i < len(data):
        value, i = read_varint(data, i)
        values.append(value)
    return values


def pois(tile):
    """Each single point of the tile's `pois` layer as (id, properties, x, y, extent)."""
    for number, layer in fields(tile):
        if number != 3: continue
        name, extent, keys, values, features = None, 4096, [], [], []
        for field, value in fields(layer):
            if field == 1: name = value.decode()
            elif field == 2: features.append(value)
            elif field == 3: keys.append(value.decode())
            elif field == 4: values.append(next((v.decode() for f, v in fields(value) if f == 1), None))
            elif field == 5: extent = value
        if name != "pois": continue
        for feature in features:
            parts = dict(fields(feature))
            geometry = packed(parts.get(4, b""))
            # A single point is one MoveTo command with one zigzag-encoded coordinate pair.
            if parts.get(3) != 1 or len(geometry) != 3 or geometry[0] != 9: continue
            tags = packed(parts.get(2, b""))
            properties = {keys[k]: values[v] for k, v in zip(tags[::2], tags[1::2])}
            x, y = ((v >> 1) ^ -(v & 1) for v in geometry[1:])
            yield parts.get(1), properties, x, y, extent


def varint(value):
    out = bytearray()
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def encode(number, value):
    """One protobuf field: an integer as a varint, bytes as a length-delimited payload."""
    if isinstance(value, int): return varint(number << 3) + varint(value)
    return varint(number << 3 | 2) + varint(len(value)) + value


def tile(places):
    """A vector tile with one `pois` layer of (id, properties, x, y) points."""
    keys, values, features = {}, {}, b""
    for identity, properties, x, y in places:
        tags = []
        for key, value in properties.items():
            tags += [keys.setdefault(key, len(keys)), values.setdefault(value, len(values))]
        point = varint(9) + varint(x << 1) + varint(y << 1)
        features += encode(2, encode(1, identity) + encode(2, b"".join(map(varint, tags))) + encode(3, 1) + encode(4, point))
    layer = (encode(15, 2) + encode(1, b"pois") + features + b"".join(encode(3, key.encode()) for key in keys)
             + b"".join(encode(4, encode(1, value.encode())) for value in values) + encode(5, EXTENT))
    return encode(3, layer)


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
            (identity, properties, math.floor((x % 1) * EXTENT), math.floor((y % 1) * EXTENT)))
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
