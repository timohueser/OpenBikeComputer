"""The TrailView feature tiles of a region: `trailview-features.pmtiles`, as
`specs/trailview-features.md` defines it.

The `obc data` step `planner/trailview-features` reads the OSM of `planner/osm`, the master terrain of
the grid of `planner/terrain` and the kind table `data/trailview/kinds.json` from its options. Osmium selects the
objects and assembles the areas; `trailview/roads.py` sets the heights of roads, `trailview/water.py`
the levels of rivers and lakes, `trailview/lifts.py` adds the pylons of lifts."""

import argparse
from functools import lru_cache
import gzip
import json
import math
from pathlib import Path
import re
import subprocess

from . import region_osm, step_request
from .trailview import lifts, profiles, structures, water

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
    word = lambda text: isinstance(text, str) and text != ""
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
                      f"{key!r} takes \"*\" or a list of unique values")
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


def selected_by(entry, tags):
    """The tags through which a selector of `entry` matches."""
    return {(key, tags[key]) for selector in entry["match"] if matches(selector, tags) for key in selector}


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
            # Osmium gives a closed way as a line and as an area. When one tag selects it for both, as
            # `waterway=dam` does, it is an area.
            if kind == "line" and coordinates[0] == coordinates[-1] and tags.get("area") != "no":
                area = kind_of(kinds, "area", tags)
                if area and selected_by(entry, tags) & selected_by(area, tags):
                    continue
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
    """Douglas-Peucker on each part (outer, points, heights). The anchors, vertices that other features
    share, and the ends of a line stay: junctions and the joins of rivers and lakes keep their places.
    The rings of an area then take back vertices until no two of their edges cross."""
    if geometry == "point":
        return parts
    sequences = []
    for (outer, points, _), fixed in zip(parts, anchors):
        if geometry == "line":
            stops = sorted({0, len(points) - 1, *fixed})
        else:
            start = (fixed or [0])[0]
            stops = sorted({(index - start) % len(points) for index in fixed} | {0, len(points)})
            points = points[start:] + points[:start + 1]
        sequences.append((points, reduced(points, stops, tolerance)))
    if geometry == "area":
        untangle(sequences)
    end = -1 if geometry == "area" else None
    result = []
    for (outer, original, z), (points, kept), fixed in zip(parts, sequences, anchors):
        # A ring was turned to start at its first anchor.
        start = (fixed or [0])[0] if geometry == "area" else 0
        indices = [(start + index) % len(original) for index in kept[:end]]
        result.append((outer, [original[index] for index in indices], z and [z[index] for index in indices]))
    return result


def farthest(points, i, j):
    """The index between i and j of the point farthest from the chord i-j, or None, and its distance."""
    (ax, ay), (bx, by) = points[i], points[j]
    dx, dy = bx - ax, by - ay
    length = math.hypot(dx, dy)
    distance, index = -1.0, None
    for k in range(i + 1, j):
        px, py = points[k]
        d = abs(dx * (ay - py) - dy * (ax - px)) / length if length else math.hypot(px - ax, py - ay)
        if d > distance:
            distance, index = d, k
    return index, distance


def reduced(points, stops, tolerance):
    """The sorted indices that Douglas-Peucker keeps between the stops."""
    keep, spans = set(stops), list(zip(stops, stops[1:]))
    while spans:
        i, j = spans.pop()
        index, distance = farthest(points, i, j)
        if index is not None and distance > tolerance:
            keep.add(index)
            spans += [(i, index), (index, j)]
    return sorted(keep)


def untangle(sequences):
    """Give each edge that crosses another edge of the rings the vertex at which Douglas-Peucker
    would split it next, until no edges cross. Rings without crossings at full detail end without
    crossings."""
    while True:
        added = set()
        for ring, edge in crossings(sequences):
            points, kept = sequences[ring]
            index, _ = farthest(points, kept[edge], kept[edge + 1])
            if index is not None:
                added.add((ring, index))
        if not added:
            return
        for ring, index in added:
            sequences[ring][1].append(index)
        for _, kept in sequences:
            kept.sort()


def crosses(a, b, c, d):
    side = lambda p, q, r: (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    return side(a, b, c) * side(a, b, d) < 0 and side(c, d, a) * side(c, d, b) < 0


def crossings(sequences):
    """(ring, edge) of each kept edge that crosses another one. A grid of about one edge per cell
    finds the pairs to test."""
    edges = [(ring, n, points[kept[n]], points[kept[n + 1]])
             for ring, (points, kept) in enumerate(sequences) for n in range(len(kept) - 1)]
    if len(edges) < 4:
        return set()
    xs, ys = [x for *_, a, b in edges for x in (a[0], b[0])], [y for *_, a, b in edges for y in (a[1], b[1])]
    cell = max(max(xs) - min(xs), max(ys) - min(ys)) / math.isqrt(len(edges)) or 1.0
    grid = {}
    for number, (*_, (ax, ay), (bx, by)) in enumerate(edges):
        for column in range(int(min(ax, bx) // cell), int(max(ax, bx) // cell) + 1):
            for row in range(int(min(ay, by) // cell), int(max(ay, by) // cell) + 1):
                grid.setdefault((column, row), []).append(number)
    found = set()
    for members in grid.values():
        for position, first in enumerate(members):
            for second in members[position + 1:]:
                (r1, n1, a, b), (r2, n2, c, d) = edges[first], edges[second]
                if len({a, b, c, d}) == 4 and crosses(a, b, c, d):
                    found |= {(r1, n1), (r2, n2)}
    return found


def meets(a, b, rect):
    """Whether the segment a-b meets `rect` (x0, y0, x1, y1), by Liang-Barsky."""
    (ax, ay), (bx, by), (x0, y0, x1, y1) = a, b, rect
    dx, dy = bx - ax, by - ay
    enter, leave = 0.0, 1.0
    for p, q in ((-dx, ax - x0), (dx, x1 - ax), (-dy, ay - y0), (dy, y1 - ay)):
        if p == 0:
            if q < 0:
                return False
        elif p < 0:
            enter = max(enter, q / p)
        else:
            leave = min(leave, q / p)
    return enter <= leave


def clip_line(points, rect):
    """The pieces of a polyline that meet `rect`, as lists of vertex indices. A piece keeps the segments
    that meet `rect` whole, and one more vertex at each end: the tiles that hold a segment hold the
    same segment and the same bends at its ends."""
    keep = set()
    for k in range(len(points) - 1):
        if meets(points[k], points[k + 1], rect):
            keep.update(range(max(0, k - 1), min(len(points), k + 3)))
    pieces, piece = [], []
    for k in range(len(points)):
        if k in keep:
            piece.append(k)
        elif piece:
            pieces.append(piece)
            piece = []
    return pieces + ([piece] if piece else [])


def clip_ring(ring, rect):
    """The vertex indices of a ring cut to `rect`, one side at a time. An edge that crosses a side
    stays whole, and the vertices outside it join directly: such a join lies outside the side, so
    inside `rect` the ring and its edges are the original ones."""
    indices = list(range(len(ring)))
    for axis, bound, side in ((0, rect[0], 1), (0, rect[2], -1), (1, rect[1], 1), (1, rect[3], -1)):
        inside = lambda index: (ring[index][axis] - bound) * side >= 0
        result = []
        for n, index in enumerate(indices):
            previous = indices[n - 1]
            if inside(index):
                result += [previous, index] if not inside(previous) else [index]
            elif inside(previous):
                result.append(index)
        indices = [index for n, index in enumerate(result) if len(result) == 1 or index != result[n - 1]]
    return indices


def clipped(geometry, parts, rect):
    """Parts (outer, points, heights) cut to `rect`. The cut makes no new vertex."""
    pick = lambda outer, points, z, indices: (outer, [points[i] for i in indices], z and [z[i] for i in indices])
    if geometry == "point":
        x0, y0, x1, y1 = rect
        return [pick(outer, points, z, [i for i, (x, y) in enumerate(points) if x0 <= x <= x1 and y0 <= y <= y1])
                for outer, points, z in parts]
    if geometry == "line":
        return [pick(False, points, z, piece) for _, points, z in parts for piece in clip_line(points, rect)]
    return [pick(outer, points, z, clip_ring(points, rect)) for outer, points, z in parts]


def twice_area(ring):
    return sum(ax * by - bx * ay for (ax, ay), (bx, by) in zip(ring, ring[1:] + ring[:1]))


def quantized(geometry, parts, left, top):
    """Parts in integer tile units. A part without enough distinct points goes, and the holes of an
    outer ring that goes go with it."""
    result, kept = [], False
    for outer, points, z in parts:
        xy, heights = [], []
        for n, (x, y) in enumerate(points):
            point = (round(x) - left, round(y) - top)
            if not xy or point != xy[-1]:
                xy.append(point)
                heights.append(z[n] if z else None)
        if geometry == "area":
            if len(xy) > 1 and xy[0] == xy[-1]:
                xy.pop()
                heights.pop()
            solid = len(xy) >= 3 and twice_area(xy) != 0
            kept = solid if outer else kept
            if not (kept and solid):
                continue
        elif len(xy) < (2 if geometry == "line" else 1):
            continue
        result.append((outer, xy, heights if z else None))
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
        x0, y0, x1, y1 = (value * n for value in extent(parts))
        return x0 < c1 + 2 and x1 > c0 - 1 and y0 < r1 + 2 and y1 > r0 - 1

    def add(self, zoom, entry, tags, parts):
        """Add the feature to each tile of `zoom` in the region whose buffered square it meets, cut to
        that square. Whether a tile took it."""
        scale = (1 << zoom) * EXTENT
        parts = [(outer, [(x * scale, y * scale) for x, y in points], z) for outer, points, z in parts]
        box = extent(parts)
        # The buffer grows towards the pole: twice that of the outer rows bounds every row's.
        margin = 2 * max(pad(zoom, int(box[1] // EXTENT) - 1), pad(zoom, int(box[3] // EXTENT) + 1))
        c0, r0, c1, r1 = self.span(zoom)
        c0, r0 = max(c0, int((box[0] - margin) // EXTENT)), max(r0, int((box[1] - margin) // EXTENT))
        c1, r1 = min(c1, int((box[2] + margin) // EXTENT)), min(r1, int((box[3] + margin) // EXTENT))
        return c0 <= c1 and r0 <= r1 and self.cut(zoom, entry, tags, parts, box, c0, r0, c1, r1)

    def cut(self, zoom, entry, tags, parts, box, c0, r0, c1, r1):
        """Cut the parts to the buffered square of the tiles c0..c1 × r0..r1, then halve the range: a
        feature over many tiles is not cut once for each of them. Whether a tile took it."""
        geometry, buffer = entry["geometry"], max(pad(zoom, r0), pad(zoom, r1))
        rect = (c0 * EXTENT - buffer, r0 * EXTENT - buffer, (c1 + 1) * EXTENT + buffer, (r1 + 1) * EXTENT + buffer)
        if box[0] > rect[2] or box[2] < rect[0] or box[1] > rect[3] or box[3] < rect[1]:
            return False
        if not (rect[0] <= box[0] and box[2] <= rect[2] and rect[1] <= box[1] and box[3] <= rect[3]):
            parts = clipped(geometry, parts, rect)
            if (box := extent(parts)) is None:
                return False
        if (c0, r0) == (c1, r1):
            cut = quantized(geometry, parts, c0 * EXTENT, r0 * EXTENT)
            if cut:
                self.tiles.setdefault((zoom, c0, r0), Tile()).add(entry["kind"], tags, geometry, cut)
            return bool(cut)
        if c1 - c0 >= r1 - r0:
            middle = (c0 + c1) // 2
            halves = ((c0, r0, middle, r1), (middle + 1, r0, c1, r1))
        else:
            middle = (r0 + r1) // 2
            halves = ((c0, r0, c1, middle), (c0, middle + 1, c1, r1))
        return any([self.cut(zoom, entry, tags, parts, box, *half) for half in halves])


def extent(parts):
    """The box (x0, y0, x1, y1) of the points of the parts, or None when they have none."""
    xs, ys = [x for _, points, _ in parts for x, _ in points], [y for _, points, _ in parts for _, y in points]
    return (min(xs), min(ys), max(xs), max(ys)) if xs else None


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


def bake(kinds, pbf, bounds, output, attribution, terrain=None, classes=None):
    """Write the archive of the region `bounds` from the OSM `pbf`, with the heights of roads, rivers and
    lakes from the `profiles.Terrain` when there is one, by the road `classes` (else those of
    `roads.TABLE`); return the metrics."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer
    import numpy as np
    # The road solver and the rails need scipy, which the readers of the tiles do not have.
    from .trailview import rails, roads

    classes = roads.CLASSES if classes is None else roads.validate(classes)

    binary, version = region_osm.probe()
    tiles, low, shared, counts, held, metrics, river_areas = Tiles(bounds), [], {}, {}, [], {}, []

    def place(entry, tags, parts):
        if tiles.add(MAX_ZOOM, entry, tags, parts):
            counts[entry["kind"]] = counts.get(entry["kind"], 0) + 1
        if entry.get("min_zoom", MAX_ZOOM) < MAX_ZOOM and tiles.meets(entry["min_zoom"], parts):
            low.append((entry, tags, parts))
            for _, points, _ in parts:
                for point in points:
                    shared[point] = shared.get(point, 0) + 1

    # The lifts wait for the other lifts, and the buildings for the lifts: a station shares the place of
    # another lift's end and replaces a building.
    ways, buildings, lift_entry = [], [], None
    for entry, tags, parts in objects(kinds, pbf, binary):
        if lifts.solves(entry["kind"], tags):
            lift_entry = entry
            ways += [(tags, points) for _, points in parts]
            continue
        parts = [(outer, points, None) for outer, points in parts]
        if entry["kind"] == "building":
            buildings.append((entry, tags, parts))
            continue
        solver = terrain and tiles.meets(MAX_ZOOM, parts) and next((m for m in (water, roads, rails) if m.solves(entry["kind"], tags)), None)
        if solver:
            held.append((solver, entry, tags, parts))
        else:
            place(entry, tags, parts)
            if entry["kind"] == "river-area":
                river_areas.append([points for _, points, _ in parts])
    rings = [[np.array(points) for outer, points, _ in parts if outer] for *_, parts in buildings]
    rigged, stations, metrics["lifts"] = lifts.rigged(ways, rings, terrain)
    del rings
    for tags, points in rigged:
        place(lift_entry, tags, [(False, points, None)])
    for n, (entry, tags, parts) in enumerate(buildings):
        if n not in stations:
            place(entry, tags, parts)
    del buildings
    # The heights of a junction need every road that meets there, a river level its whole network, a ford
    # the water it crosses, and a platform its tracks.
    tracks = []
    if held:
        waters = [(entry, tags, parts) for solver, entry, tags, parts in held if solver is water]
        ways = [(entry, tags, parts) for solver, entry, tags, parts in held if solver is roads]
        levels, metrics["water"] = water.solve([(entry["kind"], tags, parts) for entry, tags, parts in waters], terrain)
        rivers = [(points, z) for (entry, _, parts), heights in zip(waters, levels) if entry["kind"] in water.RIVERS
                  for (_, points, _), z in zip(parts, heights) if z]
        for (entry, tags, parts), heights in zip(waters, levels):
            place(entry, tags, [(outer, points, z) for (outer, points, _), z in zip(parts, heights)])
        if ways:
            heights, culverts, underpasses, metrics["roads"] = roads.solve([(entry["kind"], tags, parts) for entry, tags, parts in ways], terrain, rivers, classes)
            # A culvert is a road: the stamps build it over the ground.
            ways = [(entry, {k: v for k, v in tags.items() if k != "bridge"} if n in culverts else tags, parts) for n, ((entry, tags, _), parts) in enumerate(zip(ways, heights))]
            decks = [(n, entry["kind"], tags, parts[0]) for n, (entry, tags, parts) in enumerate(ways) if structures.is_bridge(entry["kind"], tags) and parts[0][2]]
            waterways = [(water.half_width(tags), points) for (entry, tags, parts) in waters if entry["kind"] in water.RIVERS for _, points, _ in parts]
            lakes = [[points for _, points, _ in parts] for entry, _, parts in waters if entry["kind"] in water.LAKES]
            under = [(structures.road_half(tags), points) for entry, tags, parts in ways if not structures.is_bridge(entry["kind"], tags) for _, points, _ in parts]
            added, piers, metrics["bridges"] = structures.bridges([(kind, tags, part[1], part[2]) for _, kind, tags, part in decks], waterways, lakes + river_areas, terrain, under)
            extra = {n: tags for (n, *_), tags in zip(decks, added)}
            for n, (entry, tags, parts) in enumerate(ways):
                if entry["kind"] == "gallery" and parts[0][2]:
                    extra[n] = {"uphill": structures.uphill(parts[0][1], terrain)}
                if entry["kind"] == "tunnel" and tags.get("tunnel") != "building_passage":
                    kept = [part for part in parts if not (part[2] and structures.floating(part[1], part[2], terrain))]
                    metrics["roads"]["floating_tunnels"] = metrics["roads"].get("floating_tunnels", 0) + len(parts) - len(kept)
                    if not kept:
                        continue
                    parts = kept
                if (entry["kind"] == "tunnel" and tags.get("tunnel") != "building_passage" and n not in underpasses
                        and all(structures.shallow(tags, points, z, terrain) for _, points, z in parts if z)):
                    # A tunnel under what the terrain leaves out, such as a building, is a road on the ground.
                    tags = {k: v for k, v in tags.items() if k != "tunnel"}
                    entry = kind_of(kinds, "line", {**tags, "tunnel": "no"}) or entry
                    metrics["roads"]["shallow_tunnels"] = metrics["roads"].get("shallow_tunnels", 0) + 1
                place(entry, {**tags, **extra.get(n, {})}, parts)
                if structures.is_track(tags):
                    tracks += [(entry["kind"], tags, part) for part in parts if part[2]]
            for point, z, way in piers:
                place({"kind": "bridge-pier", "geometry": "point", "min_zoom": ways[decks[way][0]][0].get("min_zoom", MAX_ZOOM)}, {}, [(False, [point], [z])])
        outside = [(tags, points, z) for kind, tags, (_, points, z) in tracks if kind != "tunnel"]
        wired = [(tags, points, z, structures.is_bridge(kind, tags)) for kind, tags, (_, points, z) in tracks if kind == "railway" and rails.electrified(tags)]
        for point, z, tags in rails.masts(wired, outside):
            place({"kind": "catenary-mast", "geometry": "point"}, tags, [(False, [point], [z])])
        stations = [(entry, tags, parts) for solver, entry, tags, parts in held if solver is rails]
        heights = rails.platforms([parts for _, _, parts in stations], outside)
        for (entry, tags, parts), solved in zip(stations, heights):
            place(entry, tags, solved or parts)
        metrics["rails"] = {"tracks": len(tracks), "masts": counts.get("catenary-mast", 0), "platforms_unknown": sum(h is None for h in heights)}
    for entry, tags, parts in low:
        anchors = [[index for index, point in enumerate(points) if shared[point] > 1] for _, points, _ in parts]
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
        }, {"features_format": FORMAT, "extent": EXTENT, "buffer_m": BUFFER_M, "attribution": attribution,
            "bounds": bounds})
    size = output.stat().st_size
    return {"osmium": version.splitlines()[0], "kinds": counts, "tiles": len(tiles.tiles), "bytes": size,
            "bytes_per_km2": round(size / area_km2(bounds)), **{k: v for k, v in metrics.items() if v}}


def step(request):
    options, output = request["options"], Path(request["output"])
    (pbf,) = request["layers"]["planner/osm"].values()
    terrain = profiles.Terrain.grid(request["layers"]["planner/terrain/grid"])
    # Osmium export reads the file twice and finds its format by the file name; a store object has none.
    osm = step_request.view({"osm.pbf": pbf}, output.with_name("planner-osm")) / "osm.pbf"
    archive = output / "trailview-features.pmtiles"
    metrics = bake(validate(options["kinds"]), osm, options["bounds"], archive, options["attribution"], terrain, options["roads"])
    step_request.metrics(request, metrics)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", help="run as the obc data step, with its request on standard input")
    parser.add_argument("--osm", type=Path, help="bake a box without obc data: the OSM extract of the box")
    parser.add_argument("--terrain", type=Path, help="a planner-dem MBTiles of the box")
    parser.add_argument("--bounds", help="west,south,east,north")
    parser.add_argument("--output", type=Path, help="the archive to write")
    args = parser.parse_args()
    if args.step:
        return step(step_request.read())
    if not (args.osm and args.terrain and args.bounds and args.output):
        parser.error("name --step, or --osm, --terrain, --bounds and --output")
    bounds = [float(value) for value in args.bounds.split(",")]
    metrics = bake(validate(json.loads(TABLE.read_text())), args.osm, bounds, args.output, "© OpenStreetMap contributors",
                   profiles.Terrain.mbtiles(args.terrain))
    print(json.dumps(metrics, indent=1))


if __name__ == "__main__":
    main()
