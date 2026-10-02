"""Derive the route network and access tiles from the overlay index of the routing package."""

import argparse
from array import array
from contextlib import closing
import gzip
import json
import math
from pathlib import Path
import sqlite3

from . import planner_mvt as mvt

LAYERS = ("cycling", "hiking", "access", "routes")
# The basemap's deepest zoom. The planner draws deeper zooms from these tiles.
MAX_ZOOM = 14
# Tile units around each tile: line joins and caps at the edge draw without a seam.
BUFFER = 64
# Half a screen pixel of a 512 pixel tile; the deepest zoom keeps the full precision for overzoom.
TOLERANCE = 4


def coordinates(blob):
    """The Postcard `Vec<[i32;2]>` of the overlay index as flat Web Mercator pairs from 0 to 1."""
    count, i = mvt.read_varint(blob, 0)
    values = []
    for _ in range(count * 2):
        value, i = mvt.read_varint(blob, i)
        values.append(value >> 1 ^ -(value & 1))
    if i != len(blob): raise ValueError("Trailing overlay coordinate data")
    points, lon, lat = array("d"), 0, 0
    for dlon, dlat in zip(values[::2], values[1::2]):
        lon, lat = lon + dlon, lat + dlat
        points.append((lon / 1e6 + 180) / 360)
        points.append((1 - math.asinh(math.tan(math.radians(lat / 1e6))) / math.pi) / 2)
    return points


def properties(kind, attributes, routes, visible):
    """The tile properties of one feature and the IDs of the routes it names.

    A route line names only the routes that its zoom draws. The tile's `routes` layer holds their details once."""
    if kind == "access":
        return {key: json.dumps(value, separators=(",", ":"), sort_keys=True) if isinstance(value, (list, dict)) else value
                for key, value in attributes.items()}, []
    members = [routes[identity] for identity in attributes["routes"] if visible(routes[identity]["rank"])]
    result = {"rank": attributes["rank"], "ref": attributes["ref"], "routes": json.dumps([route["id"] for route in members])}
    # Members are in rank order; the first route with a trail blaze marks the way.
    marker = next((route["symbol"] for route in members if route["symbol"]), "")
    if kind == "hiking" and marker: result["marker"] = marker
    return result, [route["id"] for route in members]


def simplify(xs, ys, tolerance):
    """Douglas-Peucker on the integer units of one zoom; it keeps the end points."""
    keep = [False] * len(xs)
    keep[0] = keep[-1] = True
    pending = [(0, len(xs) - 1)]
    limit = tolerance * tolerance
    while pending:
        first, last = pending.pop()
        ax, ay, dx, dy = xs[first], ys[first], xs[last] - xs[first], ys[last] - ys[first]
        length = dx * dx + dy * dy
        furthest, distance = None, limit
        for i in range(first + 1, last):
            x, y = xs[i] - ax, ys[i] - ay
            t = min(1, max(0, (x * dx + y * dy) / length)) if length else 0
            d = (x - dx * t) ** 2 + (y - dy * t) ** 2
            if d > distance: furthest, distance = i, d
        if furthest is not None:
            keep[furthest] = True
            pending += [(first, furthest), (furthest, last)]
    line = [(round(x), round(y)) for x, y, k in zip(xs, ys, keep) if k]
    return [p for i, p in enumerate(line) if not i or p != line[i - 1]]


def tiles_of(points, zoom):
    """Each tile of the zoom that the line crosses, with its runs in tile units. Each segment is clipped alone."""
    scale = (1 << zoom) * mvt.EXTENT
    line = simplify([v * scale for v in points[0::2]], [v * scale for v in points[1::2]],
                    1 if zoom == MAX_ZOOM else TOLERANCE)
    tiles = {}
    for (x0, y0), (x1, y1) in zip(line, line[1:]):
        for tx in range((min(x0, x1) - BUFFER) // mvt.EXTENT, (max(x0, x1) + BUFFER) // mvt.EXTENT + 1):
            for ty in range((min(y0, y1) - BUFFER) // mvt.EXTENT, (max(y0, y1) + BUFFER) // mvt.EXTENT + 1):
                ax, ay, dx, dy = x0 - tx * mvt.EXTENT, y0 - ty * mvt.EXTENT, x1 - x0, y1 - y0
                low, high = -BUFFER, mvt.EXTENT + BUFFER
                t0, t1 = 0.0, 1.0
                # Liang-Barsky: the part of the segment inside the buffered tile.
                for p, q in ((-dx, ax - low), (dx, high - ax), (-dy, ay - low), (dy, high - ay)):
                    if p == 0:
                        if q < 0: t0 = 2
                    elif p < 0: t0 = max(t0, q / p)
                    else: t1 = min(t1, q / p)
                if t0 > t1: continue
                start = (round(ax + t0 * dx), round(ay + t0 * dy))
                end = (round(ax + t1 * dx), round(ay + t1 * dy))
                runs = tiles.setdefault((tx, ty), [])
                if not runs or runs[-1][-1] != start: runs.append([start])
                if end != runs[-1][-1]: runs[-1].append(end)
    return {tile: [run for run in runs if len(run) > 1] for tile, runs in tiles.items() if any(len(run) > 1 for run in runs)}


def reverse(points):
    return array("d", (v for i in range(len(points) - 2, -1, -2) for v in points[i:i + 2]))


def merge(lines):
    """Join (id, points) lines through each node where exactly two of them end. A chain keeps its first ID."""
    ends = {}
    for index, (_, points) in enumerate(lines):
        for node in (tuple(points[:2]), tuple(points[-2:])): ends.setdefault(node, []).append(index)
    used = [False] * len(lines)
    for first, (identity, points) in enumerate(lines):
        if used[first]: continue
        used[first] = True
        sides = []
        for node in (tuple(points[-2:]), tuple(points[:2])):
            pieces = []
            while len(ends[node]) == 2 and (other := next((i for i in ends[node] if not used[i]), None)) is not None:
                used[other] = True
                piece = lines[other][1]
                if tuple(piece[:2]) != node: piece = reverse(piece)
                pieces.append(piece)
                node = tuple(piece[-2:])
            sides.append(pieces)
        chain = array("d")
        for piece in reversed(sides[1]): chain += reverse(piece)[:-2]
        chain += points
        for piece in sides[0]: chain += piece[2:]
        yield identity, chain


def derive(index, destination):
    """Write one vector tile pyramid with the `cycling`, `hiking`, `access` and `routes` layers of the overlay index."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import write

    with closing(sqlite3.connect(f"{index.resolve().as_uri()}?mode=ro", uri=True)) as db:
        package, coverage = db.execute("SELECT package, coverage FROM metadata").fetchone()
        routes = {identity: json.loads(value) for identity, value in db.execute("SELECT id, properties FROM routes")}
        geometries, attributes, features, zooms = {}, {}, [], {}
        for kind, cycling, walking, geometry, way, blob, attribute, value in db.execute(
                """SELECT f.kind, f.cycling_minzoom, f.walking_minzoom, g.id, g.way, g.coordinates, a.id, a.properties
                   FROM features f JOIN geometries g ON g.id=f.geometry JOIN attributes a ON a.id=f.attributes ORDER BY f.id"""):
            # Deeper features ride in the deepest tiles; the planner's filters show them from their own zoom.
            minimum = min(MAX_ZOOM, math.floor(min(z for z in (cycling, walking) if z is not None)))
            if geometry not in geometries: geometries[geometry] = coordinates(blob)
            if attribute not in attributes: attributes[attribute] = json.loads(value)
            # The overlay index chooses the zoom of each rank: a route line appears with its highest ranked route.
            if kind != "access": zooms[attributes[attribute]["rank"]] = minimum
            features.append((minimum, kind, way, attribute, geometry))
    min_zoom = min(f[0] for f in features)
    tiles = {}
    for zoom in range(min_zoom, MAX_ZOOM + 1):
        visible = lambda rank: zooms.get(rank, max(zooms.values())) <= zoom
        values, groups, lines = {}, {}, []
        for minimum, kind, way, attribute, geometry in features:
            if minimum > zoom: continue
            if (kind, attribute) not in values: values[kind, attribute] = properties(kind, attributes[attribute], routes, visible)
            tags, members = values[kind, attribute]
            if kind == "access": lines.append((kind, way, tags, members, geometries[geometry]))
            else: groups.setdefault((kind, json.dumps(tags, sort_keys=True)), (tags, members, []))[2].append((way, geometries[geometry]))
        # A route line has no way details, so one feature draws a whole stretch of equal routes.
        for (kind, _), (tags, members, group) in groups.items():
            lines += [(kind, identity, tags, members, points) for identity, points in merge(group)]
        layers, named = {}, {}
        for kind, identity, tags, members, points in lines:
            for tile, runs in tiles_of(points, zoom).items():
                layers.setdefault(tile, {}).setdefault(kind, mvt.Layer(kind)).add(identity, tags, mvt.LINESTRING, mvt.line(runs))
                named.setdefault(tile, set()).update(members)
        for tile, identities in named.items():
            catalogue = layers[tile]["routes"] = mvt.Layer("routes")
            for identity in sorted(identities):
                route = routes[identity]
                catalogue.add(identity, {k: v for k, v in route.items() if k != "id" and v not in ("", None)}, mvt.POINT, [9, 0, 0])
        for (x, y), content in layers.items():
            data = b"".join(content[name].encode() for name in LAYERS if name in content)
            tiles[zxy_to_tileid(zoom, x, y)] = gzip.compress(data, mtime=0)
        print(f"Overlay zoom {zoom}: {len(layers)} tiles", flush=True)
    west, south, east, north = json.loads(coverage)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with write(destination) as writer:
        for tile in sorted(tiles):
            writer.write_tile(tile, tiles[tile])
        e7 = lambda value: round(value * 1e7)
        writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                         "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                         "center_zoom": min_zoom, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2)}, {
            "name": "OpenBikeComputer route networks and access",
            "attribution": '<a href="https://www.openstreetmap.org/copyright">Route networks & access © OpenStreetMap</a>',
            "routing_package": package,
            "vector_layers": [{"id": name, "minzoom": min_zoom, "maxzoom": MAX_ZOOM} for name in LAYERS]})
    return {"tiles": len(tiles), "bytes": sum(map(len, tiles.values()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("index", type=Path, help="routing/overlays.sqlite")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(derive(args.index, args.output))


if __name__ == "__main__":
    main()
