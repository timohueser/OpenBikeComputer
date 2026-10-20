"""The TrailView feature tiles of a region: `trailview-features.pmtiles`, as
`specs/trailview-features.md` defines it.

The `obc data` step `planner/trailview-features` reads the OSM of `planner/osm`, and the kind table
`data/trailview/kinds.json` from its options. Osmium selects the objects and assembles the areas."""

import argparse
from functools import lru_cache
import gzip
import json
import math
from pathlib import Path
import re
import subprocess

from . import region_osm, step_request

FORMAT = 1
EXTENT = 8192
MIN_ZOOM, MAX_ZOOM = 8, 14
# The widest stamp, a road cone, reaches 25 m past the road edge: 40 m holds it for a road up to
# 30 m wide.
BUFFER_M = 40.0
# Below MAX_ZOOM a tile drops detail under a quarter texel of a 512-texel terrain node.
TOLERANCE = EXTENT / 2048
EQUATOR_M = 40075016.686
EARTH_RADIUS_KM = 6371.0088
GEOMETRIES = ("point", "line", "area")
HEIGHTS = ("surface", "pad", "profile", "none")
FIELDS = {"kind", "geometry", "match", "keep", "height", "min_zoom", "active"}
EXPORTED = {"Point": "point", "LineString": "line", "MultiPolygon": "area"}
TABLE = Path(__file__).resolve().parent.parent / "data/trailview/kinds.json"


def validate(kinds):
    """The kind table, or a ValueError that names the first entry that breaks a rule of the spec."""
    if not isinstance(kinds, list) or not kinds:
        raise ValueError("The kind table is a list of entries")
    names = set()
    word = lambda text: isinstance(text, str) and text != "" and not set(text) & set(",=")
    for entry in kinds:
        name = entry.get("kind") if isinstance(entry, dict) else None

        def check(condition, rule):
            if not condition:
                raise ValueError(f"Kind {name!r}: {rule}")

        check(isinstance(name, str) and re.fullmatch(r"[a-z][a-z0-9]*(-[a-z0-9]+)*", name), "the name is kebab-case")
        check(name not in names, "the name is unique")
        names.add(name)
        check(entry.keys() <= FIELDS, f"the fields are {sorted(FIELDS)}")
        check(entry.get("geometry") in GEOMETRIES, f"the geometry is one of {GEOMETRIES}")
        check(entry.get("height") in HEIGHTS, f"the height is one of {HEIGHTS}")
        zoom = entry.get("min_zoom", MAX_ZOOM)
        check(type(zoom) is int and MIN_ZOOM <= zoom <= MAX_ZOOM, f"min_zoom is {MIN_ZOOM} to {MAX_ZOOM}")
        check(type(entry.get("active", True)) is bool, "active is true or false")
        match = entry.get("match")
        check(isinstance(match, list) and match, "match is a list of selectors")
        for selector in match:
            check(isinstance(selector, dict) and selector, "a selector names at least one key")
            for key, values in selector.items():
                check(word(key) and (values == "*" or isinstance(values, list) and values
                                     and all(map(word, values)) and len(set(values)) == len(values)),
                      f"{key!r} takes \"*\" or a list of unique values without ',' or '='")
        keep = entry.get("keep")
        check(isinstance(keep, list) and all(map(word, keep)) and len(set(keep)) == len(keep),
              "keep is a list of unique keys")
    return kinds


def matches(selector, tags):
    return all(key in tags and (tags[key] != "no" if values == "*" else tags[key] in values)
               for key, values in selector.items())


def kind_of(kinds, geometry, tags):
    """The first entry of `geometry` with a selector that matches `tags`, active or not."""
    return next((entry for entry in kinds if entry["geometry"] == geometry
                 and any(matches(selector, tags) for selector in entry["match"])), None)


def world(lon, lat):
    """Web Mercator coordinates in [0, 1], y down."""
    return (lon + 180) / 360, (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2


def objects(kinds, pbf, binary):
    """(entry, tags, parts) of each object of an active kind. A part is (outer, points) in world
    coordinates; a ring does not repeat its first point, and each outer ring precedes its holes."""
    command = [binary, "export", pbf, "-f", "geojsonseq", "-x", "print_record_separator=false", "-o", "-"]
    with subprocess.Popen(list(map(str, command)), stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            feature = json.loads(line)
            geometry, tags = feature["geometry"], feature["properties"]
            entry = (kind := EXPORTED.get(geometry["type"])) and kind_of(kinds, kind, tags)
            if not entry or not entry.get("active", True):
                continue
            coordinates = geometry["coordinates"]
            if kind == "point":
                parts = [(False, [world(*coordinates)])]
            elif kind == "line":
                parts = [(False, [world(*point) for point in coordinates])]
            else:
                parts = [(index == 0, [world(*point) for point in ring[:-1]])
                         for polygon in coordinates for index, ring in enumerate(polygon)]
            yield entry, {key: tags[key] for key in entry["keep"] if key in tags}, parts
    if process.returncode:
        raise ValueError("Prepared Osmium export failed")


def simplified(geometry, parts, anchors, tolerance):
    """Douglas-Peucker on each part. The anchors, vertices that other features share, and the ends
    of a line stay: junctions and the joins of rivers and lakes keep their places."""
    if geometry == "point":
        return parts
    result = []
    for (outer, points), fixed in zip(parts, anchors):
        if geometry == "line":
            result.append((outer, reduced(points, sorted({0, len(points) - 1, *fixed}), tolerance)))
            continue
        start, count = (fixed or [0])[0], len(points)
        ring = points[start:] + points[:start + 1]
        stops = sorted({(index - start) % count for index in fixed} | {0, count})
        result.append((outer, reduced(ring, stops, tolerance)[:-1]))
    return result


def reduced(points, stops, tolerance):
    keep = set(stops)
    for first, last in zip(stops, stops[1:]):
        spans = [(first, last)]
        while spans:
            i, j = spans.pop()
            (ax, ay), (bx, by) = points[i], points[j]
            dx, dy = bx - ax, by - ay
            length = math.hypot(dx, dy)
            farthest, index = tolerance, None
            for k in range(i + 1, j):
                px, py = points[k]
                distance = abs(dx * (ay - py) - dy * (ax - px)) / length if length else math.hypot(px - ax, py - ay)
                if distance > farthest:
                    farthest, index = distance, k
            if index is not None:
                keep.add(index)
                spans += [(i, index), (index, j)]
    return [points[index] for index in sorted(keep)]


def clip_line(points, rect):
    """The pieces of a polyline inside `rect` (x0, y0, x1, y1), by Liang-Barsky per segment."""
    x0, y0, x1, y1 = rect
    pieces, piece = [], []
    for (ax, ay), (bx, by) in zip(points, points[1:]):
        dx, dy = bx - ax, by - ay
        enter, leave = 0.0, 1.0
        for p, q in ((-dx, ax - x0), (dx, x1 - ax), (-dy, ay - y0), (dy, y1 - ay)):
            if p == 0:
                if q < 0:
                    enter, leave = 1.0, 0.0
            elif p < 0:
                enter = max(enter, q / p)
            else:
                leave = min(leave, q / p)
        if enter > leave:
            pieces += [piece] if piece else []
            piece = []
            continue
        if not piece:
            piece = [(ax + enter * dx, ay + enter * dy)]
        piece.append((ax + leave * dx, ay + leave * dy))
        if leave < 1:
            pieces.append(piece)
            piece = []
    return pieces + ([piece] if piece else [])


def clip_ring(ring, rect):
    """A ring cut to `rect` by Sutherland-Hodgman. A concave ring can keep edges along the border."""
    for axis, bound, side in ((0, rect[0], 1), (0, rect[2], -1), (1, rect[1], 1), (1, rect[3], -1)):
        result, previous = [], ring[-1] if ring else None
        for point in ring:
            if ((point[axis] - bound) * side >= 0) != ((previous[axis] - bound) * side >= 0):
                t = (bound - previous[axis]) / (point[axis] - previous[axis])
                across = previous[1 - axis] + t * (point[1 - axis] - previous[1 - axis])
                result.append((bound, across) if axis == 0 else (across, bound))
            if (point[axis] - bound) * side >= 0:
                result.append(point)
            previous = point
        ring = result
    return ring


def clipped(geometry, parts, rect):
    x0, y0, x1, y1 = rect
    if geometry == "point":
        return [(outer, [(x, y) for x, y in points if x0 <= x <= x1 and y0 <= y <= y1]) for outer, points in parts]
    if geometry == "line":
        return [(False, piece) for _, points in parts for piece in clip_line(points, rect)]
    return [(outer, clip_ring(points, rect)) for outer, points in parts]


def twice_area(ring):
    return sum(ax * by - bx * ay for (ax, ay), (bx, by) in zip(ring, ring[1:] + ring[:1]))


def quantized(geometry, parts, left, top):
    """Parts in integer tile units. A part without enough distinct points goes, and the holes of an
    outer ring that goes go with it."""
    result, kept = [], False
    for outer, points in parts:
        xy = []
        for x, y in points:
            point = (round(x) - left, round(y) - top)
            if not xy or point != xy[-1]:
                xy.append(point)
        if geometry == "area":
            if len(xy) > 1 and xy[0] == xy[-1]:
                xy.pop()
            solid = len(xy) >= 3 and twice_area(xy) != 0
            kept = solid if outer else kept
            if not (kept and solid):
                continue
        elif len(xy) < (2 if geometry == "line" else 1):
            continue
        result.append((outer, xy, None))
    return result


@lru_cache(maxsize=None)
def pad(zoom, row):
    """The buffer of the tiles of `row` in tile units: BUFFER_M on the ground at the edge nearer the
    pole, so at least BUFFER_M everywhere."""
    n = 1 << zoom
    row = min(max(row, 0), n - 1)
    latitude = max(abs(math.atan(math.sinh(math.pi * (1 - 2 * edge / n)))) for edge in (row, row + 1))
    return BUFFER_M * n * EXTENT / (EQUATOR_M * math.cos(latitude))


class Tiles:
    """The tiles of a region from MIN_ZOOM to MAX_ZOOM that hold at least one feature."""

    def __init__(self, bounds):
        west, south, east, north = bounds
        self.region = (*world(west, north), *world(east, south))
        self.tiles = {}

    def span(self, zoom):
        n = 1 << zoom
        x0, y0, x1, y1 = (value * n for value in self.region)
        return math.floor(x0), math.floor(y0), math.ceil(x1) - 1, math.ceil(y1) - 1

    def meets(self, zoom, parts):
        """Whether the parts come within one tile of the tiles of the region at `zoom`."""
        n, (c0, r0, c1, r1) = 1 << zoom, self.span(zoom)
        xs, ys = [x * n for _, points in parts for x, _ in points], [y * n for _, points in parts for _, y in points]
        return min(xs) < c1 + 2 and max(xs) > c0 - 1 and min(ys) < r1 + 2 and max(ys) > r0 - 1

    def add(self, zoom, entry, tags, parts):
        """Add the feature to each tile of `zoom` in the region whose buffered square it meets, cut to
        that square. Whether a tile took it."""
        scale, geometry = (1 << zoom) * EXTENT, entry["geometry"]
        parts = [(outer, [(x * scale, y * scale) for x, y in points]) for outer, points in parts]
        xs, ys = [x for _, points in parts for x, _ in points], [y for _, points in parts for _, y in points]
        box = (min(xs), min(ys), max(xs), max(ys))
        # The buffer grows towards the pole: twice that of the outer rows bounds every row's.
        margin = 2 * max(pad(zoom, int(box[1] // EXTENT) - 1), pad(zoom, int(box[3] // EXTENT) + 1))
        c0, r0, c1, r1 = self.span(zoom)
        taken = False
        for row in range(max(r0, int((box[1] - margin) // EXTENT)), min(r1, int((box[3] + margin) // EXTENT)) + 1):
            buffer = pad(zoom, row)
            for column in range(max(c0, int((box[0] - margin) // EXTENT)), min(c1, int((box[2] + margin) // EXTENT)) + 1):
                rect = (column * EXTENT - buffer, row * EXTENT - buffer,
                        (column + 1) * EXTENT + buffer, (row + 1) * EXTENT + buffer)
                if box[0] > rect[2] or box[2] < rect[0] or box[1] > rect[3] or box[3] < rect[1]:
                    continue
                inside = rect[0] <= box[0] and box[2] <= rect[2] and rect[1] <= box[1] and box[3] <= rect[3]
                cut = quantized(geometry, parts if inside else clipped(geometry, parts, rect), column * EXTENT, row * EXTENT)
                if cut:
                    self.tiles.setdefault((zoom, column, row), Tile()).add(entry["kind"], tags, geometry, cut)
                    taken = True
        return taken


def varint(out, value):
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    out.append(value)


def zigzag(value):
    return value << 1 if value >= 0 else ~value << 1 | 1


class Tile:
    """The encoder of one tile. `add` takes parts as (outer, [(x, y)], z): z lists the heights in
    metres, None where a height is not solved, or is None when the part has no heights."""

    def __init__(self):
        self.strings, self.body, self.count = {}, bytearray(), 0

    def add(self, kind, tags, geometry, parts):
        body, string = self.body, lambda text: self.strings.setdefault(text, len(self.strings))
        varint(body, string(kind))
        varint(body, len(tags))
        for key in sorted(tags):
            varint(body, string(key))
            varint(body, string(tags[key]))
        solved = any(z is not None for _, _, z in parts)
        varint(body, len(parts) << 3 | solved << 2 | GEOMETRIES.index(geometry))
        x = y = height = 0
        for outer, points, z in parts:
            varint(body, len(points) << 1 | outer)
            for index, (px, py) in enumerate(points):
                varint(body, zigzag(px - x))
                varint(body, zigzag(py - y))
                x, y = px, py
                value = z[index] if solved and z is not None else None
                if solved and value is None:
                    body.append(0)
                elif solved:
                    value = round(value * 100)
                    varint(body, zigzag(value - height) + 1)
                    height = value
        self.count += 1

    def bytes(self):
        head = bytearray()
        varint(head, len(self.strings))
        for text in self.strings:
            data = text.encode()
            varint(head, len(data))
            head += data
        varint(head, self.count)
        return bytes(head + self.body)


def encode(features):
    """The bytes of a tile that holds `features`, in the form that `decode` gives."""
    tile = Tile()
    for feature in features:
        tile.add(feature["kind"], feature["tags"], feature["geometry"],
                 [(part["outer"], part["xy"], part.get("z")) for part in feature["parts"]])
    return tile.bytes()


def decode(data):
    """The features of a tile: `kind`, `tags`, `geometry` and `parts`. A part has `outer`, `xy` and,
    when the feature has heights, `z` in metres with None where a height is not solved."""
    position = 0

    def read():
        nonlocal position
        value = shift = 0
        while True:
            byte = data[position]
            position += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if byte < 0x80:
                return value

    signed = lambda value: value >> 1 ^ -(value & 1)
    strings = []
    for _ in range(read()):
        length = read()
        strings.append(bytes(data[position:position + length]).decode())
        position += length
    features = []
    for _ in range(read()):
        kind, tags = strings[read()], {}
        for _ in range(read()):
            key = strings[read()]
            tags[key] = strings[read()]
        header = read()
        solved, x, y, height, parts = header & 4, 0, 0, 0, []
        for _ in range(header >> 3):
            count = read()
            part = {"outer": bool(count & 1), "xy": [], **({"z": []} if solved else {})}
            for _ in range(count >> 1):
                x, y = x + signed(read()), y + signed(read())
                part["xy"].append([x, y])
                if solved:
                    code = read()
                    height += signed(code - 1) if code else 0
                    part["z"].append(height / 100 if code else None)
            parts.append(part)
        features.append({"kind": kind, "tags": tags, "geometry": GEOMETRIES[header & 3], "parts": parts})
    if position != len(data):
        raise ValueError("A feature tile has bytes after its last feature")
    return features


def area_km2(bounds):
    west, south, east, north = bounds
    return EARTH_RADIUS_KM ** 2 * math.radians(east - west) * (math.sin(math.radians(north)) - math.sin(math.radians(south)))


def bake(kinds, pbf, bounds, output, attribution):
    """Write the archive of the region `bounds` from the OSM `pbf`; return the metrics."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    binary, version = region_osm.probe()
    tiles, low, shared, counts = Tiles(bounds), [], {}, {}
    for entry, tags, parts in objects(kinds, pbf, binary):
        if tiles.add(MAX_ZOOM, entry, tags, parts):
            counts[entry["kind"]] = counts.get(entry["kind"], 0) + 1
        if entry.get("min_zoom", MAX_ZOOM) < MAX_ZOOM and tiles.meets(entry["min_zoom"], parts):
            low.append((entry, tags, parts))
            for _, points in parts:
                for point in points:
                    shared[point] = shared.get(point, 0) + 1
    for entry, tags, parts in low:
        anchors = [[index for index, point in enumerate(points) if shared[point] > 1] for _, points in parts]
        for zoom in range(entry["min_zoom"], MAX_ZOOM):
            tiles.add(zoom, entry, tags, simplified(entry["geometry"], parts, anchors, TOLERANCE / ((1 << zoom) * EXTENT)))
    if not tiles.tiles:
        raise ValueError("The region has no TrailView features")
    west, south, east, north = bounds
    e7 = lambda value: round(value * 1e7)
    with output.open("wb") as stream:
        writer = Writer(stream)
        for tile_id, key in sorted((zxy_to_tileid(*key), key) for key in tiles.tiles):
            writer.write_tile(tile_id, gzip.compress(tiles.tiles[key].bytes(), mtime=0))
        writer.finalize({
            "tile_type": TileType.UNKNOWN, "tile_compression": Compression.GZIP,
            "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
            "center_zoom": MAX_ZOOM, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
        }, {"features_format": FORMAT, "extent": EXTENT, "buffer_m": BUFFER_M, "attribution": attribution})
    size = output.stat().st_size
    return {"osmium": version.splitlines()[0], "kinds": counts, "tiles": len(tiles.tiles), "bytes": size,
            "bytes_per_km2": round(size / area_km2(bounds))}


def step(request):
    options = request["options"]
    (pbf,) = request["layers"]["planner/osm"].values()
    archive = Path(request["output"]) / "trailview-features.pmtiles"
    metrics = bake(validate(options["kinds"]), Path(pbf), options["bounds"], archive, options["attribution"])
    step_request.metrics(request, metrics)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
