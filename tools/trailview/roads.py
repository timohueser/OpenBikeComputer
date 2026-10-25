"""The road solver of the TrailView feature tiles: a height on each vertex of the roads, paths, railways,
tunnels and galleries, from the master terrain, by the rules per class of `data/trailview/roads.json`: the
`highway` of a road, the `railway` of a track, or `rack` for a rack railway.

A way is sampled every SAMPLE_M metres. Its heights minimise Σ wᵢ(zᵢ − hᵢ)² + λ·Σ Δgrade² / s, with Huber
weights wᵢ, so that a spike or a gully of the terrain does not move the road, and λ = smooth_m⁴ of its
class. No segment is steeper than the grade of its class. Ways that share a vertex share its height, and
a side road leaves a junction on the surface of the through road. A bridge or a tunnel ignores the terrain
under it and runs between its ends, a tunnel in a straight line; a bridge never sags under the straight line
between its ends, and rises where it would pass too low over the river, the road, the path or the track that it
crosses; a short bridge that lies on the terrain is a culvert and does not. A tunnel sinks under a road, a path or a track
that crosses over it, which does not move. Of two bridges that cross, the one of the higher layer passes over the
other, at one layer the better class, and of one class the later one. A ford lies FORD_M below the water it crosses.
Tracks side by side, as OSM maps each track of a double line, lie at one height: their beds are one bed. A
track on a ramp beside another keeps its own height."""

import json
import math
from pathlib import Path

import numpy as np
from scipy import sparse
from scipy.spatial import cKDTree
from scipy.sparse.csgraph import connected_components, dijkstra
from scipy.sparse.linalg import LinearOperator, gmres, splu

from .profiles import metres
from .structures import BORE_M, BORE_PATH_M, BORE_RAIL_M, is_track, road_half

TABLE = Path(__file__).resolve().parents[2] / "data/trailview/roads.json"
FIELDS = {"grade", "smooth_m", "tol", "reach", "least", "cut", "fill", "tilt"}
KINDS = {"main-road", "road", "path", "railway", "tunnel", "gallery"}
# The classes of the tracks.
RAILS = {"rail", "light_rail", "narrow_gauge", "rack", "funicular"}
# A cable route on rock has no bench to stamp.
UNSOLVED = {"via_ferrata"}
SAMPLE_M = 2.0
# While it solves, a way has a vertex at least every VERTEX_M; the tiles keep one only where the profile
# bends by more than KEEP_M there.
VERTEX_M, KEEP_M = 10.0, 0.2
# A side road lies on the surface of the through road for this far from their junction; a path that
# meets a path, for APPROACH_PATH_M.
APPROACH_M, APPROACH_PATH_M = 4.0, 1.5
HUBER_M = 1.0
FORD_M = 0.2
# A bridge shorter than this that lies no higher than CULVERT_RISE_M over the terrain is a road over a culvert.
CULVERT_M, CULVERT_RISE_M = 8.0, 1.0
# The least height of a deck's road over the water, the road, the path and the track that it crosses: the
# clearance under the deck and the deck's depth (DECK_M for a road, DECK_PATH_M for a path).
CLEAR_WATER_M, CLEAR_ROAD_M, CLEAR_PATH_M, CLEAR_RAIL_M = 1.5, 4.5, 2.5, 6.0
DECK_M, DECK_PATH_M = 1.0, 0.4
# A road or a path over a tunnel lies at least this far over its bore.
COVER_M = 1.0
# A track within PARALLEL_M beside another, and within PARALLEL_COS of its direction, lies at its height,
# unless the terrain under the two differs by more than PARALLEL_RISE_M within PARALLEL_FREE_M along it: a
# ramp or a flyover, whose climb the smoothing of a track spreads that far.
PARALLEL_M, PARALLEL_COS, PARALLEL_RISE_M, PARALLEL_FREE_M = 6.0, 0.9, 0.5, 100.0
# Weights against one metre of terrain samples: the terrain under a bridge or tunnel that meets no road,
# a row that pins a junction approach, a ford or a slope limit, and per metre of a bridge or tunnel the
# preference for level, so that one that meets a road at one end only is level from there.
FREE, PIN, LEVEL = 1e-4, 1e8, 1e-3
# GMRES refines each solve to REFINE_M, in at most REFINE_STEPS steps.
REFINE_M, REFINE_STEPS = 1e-6, 30
# Solves of the whole network, the first ones with the Huber weights of the solve before. Then rounds
# of the slope limits solve only the network within REGION_M of the segments whose limit changed, and
# the bridges and tunnels it meets: the terrain anchors the rest.
SOLVES, ROUNDS, REGION_M = 2, 40, 300.0
# The grade excess in metres that a segment may have before it holds its limit.
TOLERANCE_M = 1e-3
# Terrain samples per call, and the cell of the grid of waterways in world units (about 100 m).
CHUNK, GRID_UNITS = 1 << 21, 2.5e-6
# An area of `solve_by_area` is a tile of AREA_ZOOM, about 27 km at 47° N. Its job holds the ways within
# HALO_M of it.
AREA_ZOOM, HALO_M = 10, 500.0
FREE_STEP_M = 50.0
# A job holds about this many bytes per metre of its ways, most of it in the factorization of its system.
JOB_BYTES_PER_M = 300


def validate(classes):
    """The class table, or a ValueError that names the first row that breaks a rule. The table lists
    the classes from the most important down, as the junctions rank them; `path` and `road` are the
    rows of a way whose class the table lacks."""
    if not isinstance(classes, dict) or not {"road", "path"} <= classes.keys():
        raise ValueError("The road classes are an object with the rows road and path")
    for name, row in classes.items():
        number = lambda value: type(value) in (int, float) and value >= 0
        if not (isinstance(row, dict) and row.keys() == FIELDS and all(number(row[f]) for f in FIELDS - {"grade"})
                and (row["grade"] is None or number(row["grade"]) and 0 < row["grade"] < 1)):
            raise ValueError(f"Road class {name!r}: the fields are {sorted(FIELDS)}, numbers of 0 or more, and grade is null or below 1")
    return classes


CLASSES = validate(json.loads(TABLE.read_text()))


def solves(kind, tags):
    """Whether this module sets the heights of a feature."""
    return kind in KINDS and tags.get("highway") not in UNSOLVED


def class_of(tags):
    """The class of a way: `rack` for a rack railway, else the `railway` of a track or its `highway`."""
    if is_track(tags):
        return "rack" if tags.get("rack", "no") != "no" else tags["railway"]
    return tags.get("highway", "")


def rule_of(kind, tags, classes=CLASSES):
    """The row of the class table of a way: its class, else that of its kind."""
    name = class_of(tags)
    return name if name in classes else ("path" if kind == "path" else "road")


def layer(tags):
    """The OSM `layer` of a way, 0 where it has none."""
    try:
        return int(tags.get("layer", 0))
    except ValueError:
        return 0


def key(points):
    """One complex number per point, equal for equal points."""
    return points[..., 0] + 1j * points[..., 1]


def distinct(points):
    """The points of a part as an array, without a vertex that repeats the one before it: it has no length to
    solve along."""
    xy = np.asarray(points, np.float64).reshape(-1, 2)
    return xy[np.concatenate([[True], np.any(np.diff(xy, axis=0) != 0, axis=1)])] if len(xy) else xy


def is_free(kind, tags):
    """Whether a way is a bridge or a tunnel: it ignores the terrain under it."""
    return kind == "tunnel" or tags.get("bridge", "no") != "no"


class Network:
    """The ways as arrays. An OSM vertex is an occurrence of a point in a way; ways that share a point
    share its variable. Between two occurrences a way gets vertices so that no segment is longer than
    VERTEX_M, and one at the approach distance of a side road. `vertex_*` lists every vertex of every
    way in order; `a`, `b` are the variables of each segment between two vertices of a way."""

    def __init__(self, features, classes):
        names = list(classes)
        self.ways = []
        for f, (kind, tags, parts) in enumerate(features):
            rule = names.index(rule_of(kind, tags, classes))
            free = is_free(kind, tags)
            for p, (_, points, _) in enumerate(parts):
                xy = distinct(points)
                if len(xy) >= 2:
                    self.ways.append((f, p, xy, rule, free, tags.get("ford", "no") != "no"))
        if not self.ways:
            return
        counts = np.array([len(way[2]) for way in self.ways])
        self.rule = np.array([way[3] for way in self.ways])
        self.free = np.array([way[4] for way in self.ways], bool)
        self.tunnel = np.array([features[way[0]][0] == "tunnel" for way in self.ways], bool)
        self.layer = np.array([layer(features[way[0]][1]) for way in self.ways])
        self.layered = np.array(["layer" in features[way[0]][1] for way in self.ways], bool)
        # A way on the ground of a negative `layer` without a `tunnel` tag lies under something that the
        # solver does not model, such as a station hall: no way on the ground passes over or under it.
        self.indoor = ~self.free & (self.layer < 0) & np.array([features[way[0]][1].get("tunnel", "no") == "no" for way in self.ways], bool)
        self.half = np.array([road_half(features[way[0]][1]) for way in self.ways])
        self.osm_xy = np.concatenate([way[2] for way in self.ways])
        self.osm_way = np.repeat(np.arange(len(self.ways)), counts)
        self.osm_first = np.cumsum(counts) - counts
        keys, self.osm_var = np.unique(key(self.osm_xy), return_inverse=True)
        self.osm_count = len(keys)
        last = np.zeros(len(self.osm_xy), bool)
        last[self.osm_first + counts - 1] = True
        # OSM segments, from occurrence sa to sa + 1.
        sa = np.flatnonzero(~last)
        d = self.osm_xy[sa + 1] - self.osm_xy[sa]
        lengths = np.hypot(*d.T) * metres(self.osm_xy[sa, 1] + d[:, 1] / 2)
        segment_of = np.full(len(self.osm_xy), -1)
        segment_of[sa] = np.arange(len(sa))
        self.junction = np.bincount(self.osm_var) >= 2
        self.sides, self.pairs = self.junction_sides(self.junction, last, names)
        # The cuts of each OSM segment: evenly spaced, and at the approach distance of a side road.
        regular = np.maximum(1, np.ceil(lengths / VERTEX_M)).astype(np.int64)
        cut_segment = np.repeat(np.arange(len(sa)), regular - 1)
        cut_t = (np.arange(len(cut_segment)) - np.repeat(np.cumsum(regular - 1) - (regular - 1), regular - 1) + 1) / regular[cut_segment]
        occurrence, step, distance = self.sides[:3]
        s = segment_of[np.where(step == 1, occurrence, occurrence - 1)]
        long = lengths[s] > distance + 1
        approach = np.where(step == 1, distance / lengths[s], 1 - distance / lengths[s])[long]
        s = s[long]
        # The approach cuts of the two ends of a short segment closer than a metre are one.
        order = np.lexsort((approach, s))
        s, approach = s[order], approach[order]
        twin = np.zeros(len(s), bool)
        twin[1:] = (s[1:] == s[:-1]) & ((approach[1:] - approach[:-1]) * lengths[s[1:]] < 1)
        s, approach = s[~twin], approach[~twin]
        # A regular cut within a metre of an approach cut goes.
        near = np.zeros(len(cut_segment), bool)
        order = np.argsort(s, kind="stable")
        first = np.searchsorted(s[order], cut_segment, "left")
        stop = np.searchsorted(s[order], cut_segment, "right")
        for k in range(int(np.max(stop - first, initial=0))):
            at = first + k < stop
            other = approach[order[np.minimum(first + k, len(order) - 1)]]
            near |= at & (np.abs(cut_t - other) * lengths[cut_segment] <= 1)
        cut_segment = np.concatenate([cut_segment[~near], s])
        cut_t = np.concatenate([cut_t[~near], approach])
        cut_approach = np.arange(len(cut_t)) >= len(cut_t) - len(s)
        order = np.lexsort((cut_t, cut_segment))
        cut_segment, cut_t, cut_approach = cut_segment[order], cut_t[order], cut_approach[order]
        # Each occurrence moves down by the cuts of the segments before it.
        added = np.zeros(len(self.osm_xy), np.int64)
        np.add.at(added, sa[cut_segment] + 1, 1)
        self.osm_position = np.arange(len(self.osm_xy)) + np.cumsum(added)
        rank = np.arange(len(cut_segment)) - np.searchsorted(cut_segment, cut_segment, "left")
        cut_position = self.osm_position[sa[cut_segment]] + 1 + rank
        total = len(self.osm_xy) + len(cut_segment)
        self.vertex_xy = np.empty((total, 2))
        self.vertex_xy[self.osm_position] = self.osm_xy
        self.vertex_xy[cut_position] = self.osm_xy[sa[cut_segment]] + cut_t[:, None] * d[cut_segment]
        self.vertex_var = np.empty(total, np.int64)
        self.vertex_var[self.osm_position] = self.osm_var
        self.vertex_var[cut_position] = self.osm_count + np.arange(len(cut_segment))
        self.vertex_osm = np.zeros(total, bool)
        self.vertex_osm[self.osm_position] = True
        self.vertex_junction = np.zeros(total, bool)
        self.vertex_junction[self.osm_position] = self.junction[self.osm_var]
        # The tiles keep the vertices of OSM and those at an approach distance.
        self.vertex_fixed = self.vertex_osm.copy()
        self.vertex_fixed[cut_position[cut_approach]] = True
        # The OSM segment of each vertex and its place along it, 0 at its start.
        self.vertex_segment = np.full(total, -1)
        self.vertex_segment[self.osm_position[sa]] = np.arange(len(sa))
        self.vertex_segment[cut_position] = cut_segment
        self.vertex_t = np.zeros(total)
        self.vertex_t[cut_position] = cut_t
        self.count = self.osm_count + len(cut_segment)
        self.way_first = self.osm_position[self.osm_first]
        self.way_last = self.osm_position[self.osm_first + counts - 1]
        inside = np.ones(total, bool)
        inside[self.way_last] = False
        self.segment_start = np.flatnonzero(inside)
        self.a, self.b = self.vertex_var[self.segment_start], self.vertex_var[self.segment_start + 1]
        self.segment_way = np.repeat(np.arange(len(self.ways)), self.way_last - self.way_first)
        pa, pb = self.vertex_xy[self.segment_start], self.vertex_xy[self.segment_start + 1]
        self.length = np.hypot(*(pb - pa).T) * metres((pa[:, 1] + pb[:, 1]) / 2)

    def junction_sides(self, junction, last, names):
        """The side directions of the junctions, and the through road of each: its occurrence, step and
        approach metres, and per through direction (at most two) the occurrence and step, -1 for none.
        Then the through roads of two ways, as (occurrence, step) of each: the grade runs smoothly on
        across the junction, where OSM splits a way. The through road is the best class at the
        junction, and of two ways of that class, the pair that runs most nearly straight on."""
        at = np.flatnonzero(junction[self.osm_var])
        if not len(at):
            return [np.zeros(0, np.int64)] * 2 + [np.zeros(0)] + [np.zeros(0, np.int64)] * 4, np.zeros((0, 4), np.int64)
        rows = []
        for step in (1, -1):
            ok = ~last[at] if step == 1 else np.isin(at, self.osm_first, invert=True)
            o = at[ok]
            d = self.osm_xy[o + step] - self.osm_xy[o]
            rows.append((o, np.full(len(o), step), d / np.maximum(np.hypot(*d.T), 1e-300)[:, None]))
        occurrence = np.concatenate([r[0] for r in rows])
        step = np.concatenate([r[1] for r in rows])
        direction = np.concatenate([r[2] for r in rows])
        rank = self.rule[self.osm_way[occurrence]]
        order = np.lexsort((rank, self.osm_var[occurrence]))
        occurrence, step, direction, rank = occurrence[order], step[order], direction[order], rank[order]
        var = self.osm_var[occurrence]
        starts = np.flatnonzero(np.concatenate([[True], var[1:] != var[:-1]]))
        ends = np.concatenate([starts[1:], [len(var)]])
        path = names.index("path")
        out, pairs = [[] for _ in range(7)], []
        for s, e in zip(starts.tolist(), ends.tolist()):
            top = [i for i in range(s, e) if rank[i] == rank[s]]
            if len(top) >= 2:
                through = min(((float(direction[i] @ direction[k]), i, k) for n, i in enumerate(top) for k in top[n + 1:]))[1:]
            else:
                through = (top[0],)
            metres_ = APPROACH_PATH_M if rank[s] >= path else APPROACH_M
            t1, t2 = through[0], through[1] if len(through) > 1 else -1
            if t2 >= 0 and occurrence[t1] != occurrence[t2]:
                pairs.append((occurrence[t1], step[t1], occurrence[t2], step[t2]))
            for i in range(s, e):
                if i not in through:
                    for column, value in enumerate((occurrence[i], step[i], metres_, occurrence[t1], step[t1],
                                                    occurrence[t2] if t2 >= 0 else -1, step[t2] if t2 >= 0 else 0)):
                        out[column].append(value)
        return [np.array(column, float if c == 2 else np.int64) for c, column in enumerate(out)], np.array(pairs, np.int64).reshape(-1, 4)


def solve(features, terrain, rivers=(), classes=CLASSES, owned=None):
    """Per feature (kind, tags, parts): its parts (outer, points, heights) with the heights solved, None
    where the terrain is unknown, and vertices added where the profile bends between two of OSM; the
    features that are culverts; the tunnels that pass under a road or a path; and the metrics of the solve. `rivers` lists (points, levels) of the
    solved waterways, for the fords and the decks. `owned(xy)` marks the OSM vertices of the area of a job of
    `solve_by_area`: the culverts, the tunnels and the metrics count only the ways that start on one of them, and
    the vertices from one of them to the next vertex of OSM."""
    out = [[(outer, points, None) for outer, points, _ in parts] for _, _, parts in features]
    net = Network(features, classes)
    if not net.ways:
        return out, set(), set(), {}
    n, a, b, length = net.count, net.a, net.b, net.length
    total = len(net.vertex_xy)
    mine = np.ones(total, bool)
    if owned is not None:
        mine = owned(net.vertex_xy[np.maximum.accumulate(np.where(net.vertex_osm, np.arange(total), 0))])
    var_mine = np.zeros(n, bool)
    var_mine[net.vertex_var] = mine
    segment_mine = mine[net.segment_start]
    rows = [classes[name] for name in classes]
    # Samples: k per segment from its start, and the last vertex of each way. Ordered by terrain tile, so
    # that each tile decodes about once.
    k = np.maximum(1, np.ceil(length / SAMPLE_M)).astype(np.int64)
    start = net.vertex_xy[net.segment_start]
    order = np.argsort((np.floor(start[:, 1] * 8192) * 8192 + np.floor(start[:, 0] * 8192)).astype(np.int64), kind="stable")
    seg = np.repeat(order, k[order])
    t = (np.arange(len(seg)) - np.repeat(np.cumsum(k[order]) - k[order], k[order])) / k[seg]
    ends = np.cumsum(net.way_last - net.way_first) - 1
    seg, t = np.concatenate([seg, ends]), np.concatenate([t, np.ones(len(ends))])
    free = net.free[net.segment_way]
    # A bridge or a tunnel sees no terrain, unless nothing that it meets does. A bridge's end that meets no
    # other way stands on the terrain (`landings`): else the smoothness carries any rise of its deck on up.
    loose = np.zeros(n, bool)
    ends = np.concatenate([net.way_first, net.way_last])[np.tile(net.free & ~net.tunnel, 2)]
    loose[net.vertex_var[ends]] = ~net.junction[net.vertex_var[ends]]
    weight = (length / k)[seg] * (~free[seg] | loose[np.where(t == 0, a[seg], b[seg])] & ((t == 0) | (t == 1)))
    h = np.empty(len(seg))
    pa, pb = net.vertex_xy[net.segment_start], net.vertex_xy[net.segment_start + 1]
    for begin in range(0, len(seg), CHUNK):
        s, u = seg[begin:begin + CHUNK], t[begin:begin + CHUNK, None]
        xy = pa[s] + u * (pb[s] - pa[s])
        h[begin:begin + CHUNK] = terrain.heights(xy[:, 0], xy[:, 1])
    finite = np.isfinite(h)
    weight[~finite] = 0
    m = len(a)
    touched = lambda w: np.bincount(a, np.bincount(seg, w * (1 - t), m), n) + np.bincount(b, np.bincount(seg, w * t, m), n)
    on_free = np.zeros(n, bool)
    on_free[a[free]] = on_free[b[free]] = True
    # A bridge or a tunnel takes its height from the terrain that it meets, not through a road in
    # unknown terrain: the networks join only vertices with terrain samples or on a bridge or tunnel.
    linked = (touched(weight) > 0) | on_free
    joined = linked[a] & linked[b]
    _, component = connected_components(sparse.csr_matrix((np.ones(np.count_nonzero(joined)), (a[joined], b[joined])), shape=(n, n)), directed=False)
    seen = np.bincount(component, touched(weight)) > 0
    alone = free[seg] & ~seen[component[a[seg]]] & finite
    weight[alone] = (length / k)[seg[alone]] * FREE
    h[~np.isfinite(h)] = 0
    # A vertex has a height where terrain samples act on it, or where it lies on a bridge or tunnel of a
    # network that has some.
    support = touched(weight)
    known = (support > 0) | (on_free & (np.bincount(component, support) > 0)[component])

    # Rows that pin a combination of variables to a value: the approaches of side roads, and the fords.
    approach = approach_pins(net)
    fords = ford_pins(net, rivers)
    bores = chain_rows(net, net.tunnel)
    beside = parallel_pins(net, classes, terrain)
    above = clearances(net, rivers, classes, terrain)
    sags = chain_rows(net, net.free & ~net.tunnel)
    above = (np.concatenate([above[0], np.column_stack([sags[0], sags[0][:, 0]])]), np.concatenate([above[1], np.pad(sags[1], ((0, 0), (0, 1)))]),
             np.concatenate([above[2], sags[2]]), np.concatenate([above[3], np.full(len(sags[2]), -1)]))
    whole = np.all(known[above[0]], axis=1)
    C = sparse.csr_matrix((above[1].ravel(), (np.repeat(np.arange(len(above[2])), 4), above[0].ravel())), shape=(len(above[2]), n))[whole]
    least, deck_way = above[2][whole], above[3][whole]
    deck_first = above[0][whole][:, 0]
    clear = deck_way >= 0
    under = passes(net, classes)
    held_up = np.all(known[under[0]], axis=1) & np.all(known[under[2]], axis=1)
    pairs = lambda variables, coefficients: sparse.csr_matrix((coefficients.ravel(), (np.repeat(np.arange(len(variables)), 2), variables.ravel())), shape=(len(variables), n))
    # U is the upper way and L the lower at each crossing; the row of a tunnel under a road holds the tunnel alone.
    U, L, need = pairs(under[0][held_up], under[1][held_up]), pairs(under[2][held_up], under[3][held_up]), under[4][held_up]
    apart = sparse.diags(under[6][held_up].astype(float)) @ U - L
    variables = np.concatenate([approach[0], fords[0], bores[0], beside[0]])
    coefficients = np.concatenate([approach[1], fords[1], bores[1], beside[1]])
    values = np.concatenate([approach[2], fords[2], bores[2], beside[2]])
    ok = np.all(known[variables], axis=1)
    counted = ok & var_mine[variables[:, 0]]
    variables, coefficients, values = variables[ok], coefficients[ok], values[ok]
    P = sparse.csr_matrix((coefficients.ravel(), (np.repeat(np.arange(len(values)), 3), variables.ravel())), shape=(len(values), n))

    # Smoothness: per vertex inside a way, the change of grade over the mean of its two segments. A lower
    # class never moves a higher one: no row ties a side to the vertex of its junction.
    inner = np.flatnonzero(net.segment_way[:-1] == net.segment_way[1:])
    junction = net.osm_position[net.sides[0]]
    mouth = np.isin(net.segment_start[inner] + 1, np.concatenate([junction, junction + net.sides[1]]))
    inner = inner[known[a[inner]] & known[b[inner]] & known[b[inner + 1]] & ~mouth]
    triples = np.column_stack([a[inner], b[inner], b[inner + 1]])
    la, lb = length[inner], length[inner + 1]
    smooth_m = np.array([row["smooth_m"] for row in rows])
    lam = smooth_m[net.rule[net.segment_way[inner]]] ** 4
    # Across a junction the through road runs on from one way into the other: the vertex next to the
    # junction on each, and the junction.
    o1, s1, o2, s2 = net.pairs.T
    j = net.osm_position[o1]
    across = np.column_stack([net.vertex_var[j + s1], net.vertex_var[j], net.vertex_var[net.osm_position[o2] + s2]])
    span = lambda p, q: np.hypot(*(net.vertex_xy[p] - net.vertex_xy[q]).T) * metres(net.vertex_xy[q][:, 1])
    whole = np.all(known[across], axis=1)
    triples = np.concatenate([triples, across[whole]])
    la = np.concatenate([la, span(j + s1, j)[whole]])
    lb = np.concatenate([lb, span(net.osm_position[o2] + s2, j)[whole]])
    lam = np.concatenate([lam, smooth_m[net.rule[net.osm_way[o1]]][whole] ** 4]) / ((la + lb) / 2)
    D2 = sparse.csr_matrix((np.column_stack([1 / la, -1 / la - 1 / lb, 1 / lb]).ravel(),
                            (np.repeat(np.arange(len(triples)), 3), triples.ravel())), shape=(len(triples), n))
    # Grades: per segment of a class with a limit, z_b − z_a within ± grade × length. A bridge or a tunnel
    # has the grade that its ends give it: OSM joins shafts and ramps to tunnels that no limit could meet.
    # Nor has the approach of a side road, or a segment between two approaches: the through roads give
    # them their heights.
    grade = np.array([np.inf if row["grade"] is None else row["grade"] for row in rows])[net.rule[net.segment_way]]
    pinned = np.zeros(n, bool)
    pinned[approach[0][:, 0]] = True
    limited = np.flatnonzero(np.isfinite(grade) & ~free & known[a] & known[b] & ~approach[3] & ~(pinned[a] & pinned[b]))
    G = sparse.csr_matrix((np.tile([-1.0, 1.0], len(limited)), (np.repeat(np.arange(len(limited)), 2), np.column_stack([a[limited], b[limited]]).ravel())), shape=(len(limited), n))
    bound = grade[limited] * length[limited]
    level = np.flatnonzero(free & known[a] & known[b])
    L1 = sparse.csr_matrix((np.tile([-1.0, 1.0], len(level)), (np.repeat(np.arange(len(level)), 2), np.column_stack([a[level], b[level]]).ravel())), shape=(len(level), n))
    unknown = np.flatnonzero(~known)
    # The rows (A, w, c) of the least squares Σ w·(A·z − c)² that every solve holds.
    fixed = [(D2, lam, 0.0), (L1, LEVEL / np.maximum(length[level], 0.01), 0.0), (P, PIN, values),
             (sparse.csr_matrix((np.ones(len(unknown)), (np.arange(len(unknown)), unknown)), shape=(len(unknown), n)), 1.0, 0.0)]
    samples = sparse.csr_matrix((np.concatenate([1 - t, t]), (np.tile(np.arange(len(seg)), 2), np.concatenate([a[seg], b[seg]]))), shape=(len(seg), n))

    def fit(rows, z=None, moved=None):
        """The heights at the least squares of the rows (A, w, c), or z with only its heights `moved` at it. The
        factorization of the normal matrix loses a height that only weak rows hold, such as the end of a tunnel that
        meets no way, in the round-off of the PIN rows times the heights. GMRES with that factorization then solves
        against the residuals of the rows, which keep it."""
        rows = [(A if z is None else A[:, moved], np.broadcast_to(w, A.shape[:1]), c if z is None else c - A @ z) for A, w, c in rows]
        normal = sum(A.T @ sparse.diags(w) @ A for A, w, _ in rows)
        lu = splu(normal.tocsc(), permc_spec="MMD_AT_PLUS_A", diag_pivot_thresh=0.0, options={"SymmetricMode": True})
        apply = lambda v: lu.solve(sum(A.T @ (w * (A @ v)) for A, w, _ in rows))
        step = lu.solve(sum(A.T @ (w * c) for A, w, c in rows))
        rest = lu.solve(sum(A.T @ (w * (c - A @ step)) for A, w, c in rows))
        more, _ = gmres(LinearOperator(normal.shape, apply), rest, rtol=0.0, atol=REFINE_M, restart=REFINE_STEPS, maxiter=1)
        if z is None:
            return step + more
        out = z.copy()
        out[moved] += step + more
        return out

    def update(held, gz, release=True):
        """The slope limits after a solve: a segment past its limit holds it, a held one that the solve
        would make less steep lets go. Without releases the rounds end: the held segments only grow."""
        out = held.copy()
        if release:
            out[held * gz < bound] = 0
        out[gz > bound + TOLERANCE_M] = 1
        out[gz < -bound - TOLERANCE_M] = -1
        return out

    def held_rows():
        """The rows of a solve: those of `fixed`, the terrain samples and the slope limits that hold."""
        on = np.flatnonzero(held)
        return fixed + [(samples, w, h), (G[on], PIN, held[on] * bound[on])]

    held = np.zeros(len(limited))
    w = weight
    for solves_ in range(SOLVES):
        z = fit(held_rows())
        if solves_ < SOLVES - 1:
            residual = np.abs(z[a[seg]] * (1 - t) + z[b[seg]] * t - h)
            w = weight * np.minimum(1.0, HUBER_M / np.maximum(residual, 1e-9))
            # A short bridge that lies on the terrain is a culvert. Any other deck that passes too low holds
            # its least height from the next solve on.
            culvert = culverts_of(net, z, seg, t, h, finite)
            low = (C @ z < least - TOLERANCE_M) & ~(clear & culvert[deck_way])
            fixed.append((C[low], PIN, least[low]))
            # Ways that pass too near hold the margin from the next solve on; a tunnel under a road holds
            # the road's height less the margin.
            near = U @ z - L @ z < need - TOLERANCE_M
            target = np.where(under[6][held_up], need, need - U @ z)
            fixed.append((apart[near], PIN, target[near]))
        before, held = held, update(held, G @ z)
    # Rounds of the slope limits near the segments whose limit changed, the rest of the network fixed.
    # Nothing anchors a bridge or a tunnel but its ends: a region takes it whole.
    graph = sparse.csr_matrix((np.where(free, 1e-6, np.maximum(length, 0.01)), (a, b)), shape=(n, n))
    graph = graph + graph.T
    rounds = 0
    while rounds < ROUNDS and np.any(changed := held != before):
        rounds += 1
        sources = np.unique(np.concatenate([a[limited[changed]], b[limited[changed]]]))
        region = np.flatnonzero(np.isfinite(dijkstra(graph, indices=sources, limit=REGION_M, min_only=True)))
        z = fit(held_rows(), z, region)
        before, held = held, update(held, G @ z, rounds < ROUNDS // 2)

    culvert = culverts_of(net, z, seg, t, h, finite)
    keep = kept(net, z, known)
    for w_, (f, p, *_) in enumerate(net.ways):
        span = slice(net.way_first[w_], net.way_last[w_] + 1)
        mask = keep[span]
        points = list(map(tuple, net.vertex_xy[span][mask].tolist()))
        v = net.vertex_var[span][mask]
        heights = np.where(known[v], z[v], np.nan).tolist()
        out[f][p] = (out[f][p][0], points, [None if math.isnan(value) else value for value in heights])
    way_mine = mine[net.way_first]
    culverts = {net.ways[w][0] for w in np.flatnonzero(culvert & way_mine).tolist()}
    deck_mine = clear & ~culvert[deck_way] & var_mine[deck_first]
    pass_mine = var_mine[under[0][held_up][:, 0]]
    excess = (np.abs(G @ z) - bound)[segment_mine[limited]]
    limited_mine = limited[segment_mine[limited]]
    worst = limited_mine[np.argmax(excess)] if len(excess) else 0
    lon, lat = world_degrees(net.vertex_xy[net.segment_start[worst]]) if len(excess) else (0, 0)
    residual = np.abs(z[a[seg]] * (1 - t) + z[b[seg]] * t - h)
    fitted = ~free[seg] & (weight > 0) & segment_mine[seg]
    metrics = {
        "ways": int(way_mine.sum()), "variables": int(var_mine.sum()), "known": int((known & var_mine).sum()),
        "approaches": int(np.count_nonzero(counted[:len(approach[2])])),
        "fords": int(np.count_nonzero(counted[len(approach[2]):len(approach[2]) + len(fords[2])])), "culverts": len(culverts), "rounds": rounds,
        "held": int(np.count_nonzero(held[segment_mine[limited]])),
        "clearances": int(np.count_nonzero(deck_mine)),
        "clearances_low_cm": int(np.count_nonzero((C @ z < least - 0.01) & deck_mine)),
        "passes": int(np.count_nonzero(pass_mine)), "passes_near_cm": int(np.count_nonzero((U @ z - L @ z < need - 0.01) & pass_mine)),
        "grade_excess_m": round(float(np.max(excess, initial=0)), 4), "grade_excess_at": [round(lat, 5), round(lon, 5)],
        "grade_excess_over_1cm": int(np.count_nonzero(excess > 0.01)),
        "residual_p99_m": round(float(np.percentile(residual[fitted], 99)), 2) if fitted.any() else 0,
    }
    return out, culverts, {net.ways[w][0] for w in under[5][held_up & under[7]].tolist() if way_mine[w]}, metrics


def compact(kind, tags, parts):
    """A road feature past the region as `context` reads it: its points as arrays, a fraction of the memory of
    their tuples."""
    return kind, tags, [(outer, np.asarray(points, np.float64), None) for outer, points, _ in parts]


def context(features, beyond, inside):
    """The features of `beyond`, from `compact`, that the solve of `features` needs: each bridge and tunnel of a
    network of them that holds a bridge or a tunnel of `features`, and each way on the ground that a vertex
    shares with an end of such a network. Then the numbers of the features of a network with an end that meets
    no way where `inside(xy)` is false: the input cuts that network, so it has no ends to run between."""
    every, count = features + beyond, len(features)
    parts = [(f, xy) for f, (_, _, ps) in enumerate(every) for _, points, _ in ps if len(xy := distinct(points)) >= 2]
    if not parts:
        return [], set()
    free = np.array([is_free(*every[f][:2]) for f in range(len(every))], bool)
    owner = np.repeat([f for f, _ in parts], [len(xy) for _, xy in parts])
    xy = np.concatenate([xy for _, xy in parts])
    keys, vertex = np.unique(key(xy), return_inverse=True)
    on = free[owner]
    links = sparse.csr_matrix((np.ones(np.count_nonzero(on)), (owner[on], vertex[on])), shape=(len(every), len(keys)))
    _, network = connected_components(sparse.bmat([[None, links], [links.T, None]]), directed=False)
    network = network[:len(every)]
    joined = free & np.isin(network, network[:count][free[:count]])
    last = np.cumsum([len(xy) for _, xy in parts]) - 1
    tips = np.concatenate([last - [len(xy) - 1 for _, xy in parts], last])
    tips = tips[joined[owner[tips]]]
    loose = tips[(np.bincount(vertex, minlength=len(keys))[vertex[tips]] < 2) & ~inside(xy[tips])]
    whole = joined & ~np.isin(network, network[owner[loose]])
    landed = np.zeros(len(every), bool)
    landed[owner[np.isin(vertex, vertex[tips[whole[owner[tips]]]]) & ~on]] = True
    cut = set(np.flatnonzero((joined & ~whole)[:count]).tolist())
    return [beyond[f - count] for f in np.flatnonzero((whole | landed)[count:]) + count], cut


class Region:
    """The OSM vertices of the ways of the features as one graph, for the jobs of `solve_by_area`: `point` and
    `area` per vertex; `vertex` and `feature` per occurrence; the OSM segments as edges of their metres, a
    bridge or a tunnel of almost none, as in the rounds of `solve`."""

    def __init__(self, features):
        parts = [(f, distinct(points), is_free(kind, tags)) for f, (kind, tags, ps) in enumerate(features) for _, points, _ in ps]
        parts = [(f, xy, free) for f, xy, free in parts if len(xy) >= 2]
        xy = np.concatenate([xy for _, xy, _ in parts]) if parts else np.zeros((0, 2))
        counts = np.array([len(xy) for _, xy, _ in parts], np.int64)
        self.feature = np.repeat([f for f, _, _ in parts], counts).astype(np.int64)
        _, first, self.vertex = np.unique(key(xy), return_index=True, return_inverse=True)
        self.point = xy[first]
        self.area = np.floor(self.point * (1 << AREA_ZOOM)).astype(np.int64)
        way = np.repeat(np.arange(len(parts)), counts)
        start = np.flatnonzero(way[:-1] == way[1:])
        d = xy[start + 1] - xy[start]
        span = np.hypot(*d.T) * metres(xy[start, 1] + d[:, 1] / 2)
        length = np.where(np.array([free for _, _, free in parts], bool)[way[start]], 1e-6, np.maximum(span, 0.01))
        # Of two ways along one segment, the shorter edge holds.
        lo, hi = np.minimum(self.vertex[start], self.vertex[start + 1]), np.maximum(self.vertex[start], self.vertex[start + 1])
        order = np.lexsort((length, hi, lo))
        lo, hi, length = lo[order], hi[order], length[order]
        once = np.concatenate([[True], (lo[1:] != lo[:-1]) | (hi[1:] != hi[:-1])])
        lo, hi, length = lo[once], hi[once], length[once]
        n = len(self.point)
        self.graph = sparse.csr_matrix((np.concatenate([length, length]), (np.concatenate([lo, hi]), np.concatenate([hi, lo]))), shape=(n, n))
        # Per area that holds a vertex, the metres of the segments that start in it: what its job weighs.
        areas, at = np.unique(self.area, axis=0, return_inverse=True)
        weight = np.bincount(at.ravel()[self.vertex[start]], span, len(areas))
        self.areas = {tuple(area): float(m) for area, m in zip(areas.tolist(), weight)}
        self.metres = np.bincount(self.feature[start], span, len(features))
        # Points every FREE_STEP_M along each bridge and tunnel, and a vertex of its segment.
        bare = length == 1e-6
        a, b = self.point[lo[bare]], self.point[hi[bare]]
        steps = np.maximum(1, np.ceil(np.hypot(*(b - a).T) * metres(a[:, 1]) / FREE_STEP_M)).astype(np.int64)
        segment = np.repeat(np.arange(len(a)), steps + 1)
        t = (np.arange(len(segment)) - np.repeat(np.cumsum(steps + 1) - steps - 1, steps + 1)) / steps[segment]
        self.free_point = a[segment] + t[:, None] * (b - a)[segment]
        self.free_vertex = lo[bare][segment]

    def near(self, area, halo_m):
        """The features of the ways within halo_m of an area: from the nearest point of the area in a straight
        line, then along the ways."""
        n, (x, y) = 1 << AREA_ZOOM, area
        px, py = self.point.T
        dx = np.maximum(np.maximum(x / n - px, px - (x + 1) / n), 0)
        dy = np.maximum(np.maximum(y / n - py, py - (y + 1) / n), 0)
        entry = np.hypot(dx, dy) * metres(py)
        count = len(self.point)
        graph = sparse.hstack([self.graph, sparse.csr_matrix((count, 1))])
        scale = float(metres((y + 0.5) / n))
        ties = 0
        while True:
            starts = np.flatnonzero(entry <= halo_m)
            source = sparse.csr_matrix((np.maximum(entry[starts], 1e-9), (np.zeros(len(starts), np.int64), starts)), shape=(1, count + 1))
            distance = dijkstra(sparse.vstack([graph, source], format="csr"), indices=count, limit=halo_m)[:count]
            # A deck or a bore ties the ways that it crosses without a vertex, however far along it: they enter at
            # the distance of the bridge or the tunnel and their gap to it.
            free = np.isfinite(distance[self.free_vertex])
            if np.count_nonzero(free) == ties:
                return np.unique(self.feature[np.isfinite(distance)[self.vertex]]).tolist()
            ties = np.count_nonzero(free)
            gap, i = cKDTree(self.free_point[free] * scale).query(self.point * scale, distance_upper_bound=halo_m)
            tied = np.isfinite(gap)
            entry[tied] = np.minimum(entry[tied], gap[tied] + distance[self.free_vertex[free]][i[tied]])


def solve_by_area(features, terrain, rivers=(), classes=CLASSES, halo_m=HALO_M, workers=None, context=()):
    """`solve` by area, in a `step_pool.pool` of `workers`: a job solves the features within halo_m of its area,
    and each OSM vertex of the area takes its height from there, with the vertices that follow it to the next
    one. The features of `context` join the jobs, but only the areas of `features` have one. A metric adds up
    over the areas; `rounds`, `residual_p99_m` and `grade_excess_m` are the largest."""
    from .. import step_pool

    count, features = len(features), list(features) + list(context)
    region = Region(features)
    boxes = np.array([(*np.min(points, 0), *np.max(points, 0)) for points, _ in rivers]).reshape(-1, 4)
    wanted = {tuple(area) for area in np.unique(region.area[np.unique(region.vertex[region.feature < count])], axis=0).tolist()}
    jobs = sorted(wanted & region.areas.keys(), key=lambda area: -region.areas[area])
    if jobs and not workers:
        workers = step_pool.worker_count(JOB_BYTES_PER_M * int(region.metres[region.near(jobs[0], halo_m)].sum()) + 1)
    out = [[(outer, points, None) for outer, points, _ in parts] for _, _, parts in features[:count]]
    pieces, culverts, underpasses, metrics = {}, set(), set(), {}
    shared = (features, terrain, rivers, boxes, classes, region, halo_m)
    for found, culverts_, underpasses_, metrics_ in step_pool.forked(solve_area, shared, jobs, workers):
        for way, runs in found:
            pieces.setdefault(way, []).extend(runs)
        culverts |= culverts_
        underpasses |= underpasses_
        for name, value in metrics_.items():
            if name == "grade_excess_m":
                if (value, metrics_["grade_excess_at"]) > (metrics.get(name, -1.0), metrics.get("grade_excess_at", [])):
                    metrics[name], metrics["grade_excess_at"] = value, metrics_["grade_excess_at"]
            elif name in ("rounds", "residual_p99_m"):
                metrics[name] = max(metrics.get(name, 0), value)
            elif name != "grade_excess_at":
                metrics[name] = metrics.get(name, 0) + value
    for (f, p), runs in pieces.items():
        if f >= count:
            continue
        runs.sort(key=lambda run: run[0])
        out[f][p] = (out[f][p][0], [point for run in runs for point in run[1]], [z for run in runs for z in run[2]])
    return out, {f for f in culverts if f < count}, {f for f in underpasses if f < count}, metrics


def solve_area(shared, area):
    """The job of an area: per way (feature, part) that it solves, the runs (OSM vertex, points, heights) of
    its vertices in the area; the culverts, the tunnels and the metrics of the area."""
    features, terrain, rivers, boxes, classes, region, halo_m = shared
    chosen = region.near(area, halo_m)
    n, (x, y) = 1 << AREA_ZOOM, area
    owned = lambda xy: (np.floor(xy[:, 0] * n) == x) & (np.floor(xy[:, 1] * n) == y)
    xy = np.concatenate([distinct(points) for f in chosen for _, points, _ in features[f][2]])
    bounds = (*xy.min(0), *xy.max(0))
    near = [rivers[i] for i in np.flatnonzero((boxes[:, 0] <= bounds[2]) & (boxes[:, 2] >= bounds[0]) & (boxes[:, 1] <= bounds[3]) & (boxes[:, 3] >= bounds[1]))]
    out, culverts, underpasses, metrics = solve([features[f] for f in chosen], terrain, near, classes, owned)
    found = []
    for f, parts in zip(chosen, out):
        for p, (_, points, heights) in enumerate(parts):
            if heights is None:
                continue
            osm = distinct(features[f][2][p][1])
            mine, stops = owned(osm), list(map(tuple, osm.tolist()))
            # The solve keeps every OSM vertex: a point that is the next one starts its stretch.
            runs, k, last = [], 0, None
            for i, (point, z) in enumerate(zip(points, heights)):
                k += k + 1 < len(stops) and point == stops[k + 1]
                if mine[k]:
                    if last != i - 1:
                        runs.append((k, [], []))
                    runs[-1][1].append(point)
                    runs[-1][2].append(z)
                    last = i
            found.append(((f, p), runs))
    return found, {chosen[i] for i in culverts}, {chosen[i] for i in underpasses}, metrics


def world_degrees(point):
    x, y = point
    return x * 360 - 180, math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y))))


def approach_pins(net):
    """Per side direction of a junction, the rows that put its vertices within its approach distance on
    the surface of the through road: z_P − (1 − τ)·z_J − τ·z_Q = 0, with Q the next vertex of the through
    direction that the side leans along most and τ ≥ 0 the place of P along J-Q. A side pins only the
    vertices at least a metre nearer to its junction than to the next junction along its way, so that
    the two ends of a short link do not pin one vertex. Last, whether each segment lies within the
    approach distance of a junction where its way is a side. A tunnel that leaves a junction as a side
    runs straight (`chain_rows`) and has no approach."""
    sides = ~net.tunnel[net.osm_way[net.sides[0]]]
    occurrence, step, distance, t1, s1, t2, s2 = (column[sides] for column in net.sides)
    approached = np.zeros(len(net.segment_start), bool)
    if not len(occurrence):
        return np.zeros((0, 3), np.int64), np.zeros((0, 3)), np.zeros(0), approached
    segment = np.full(len(net.vertex_xy), -1)
    segment[net.segment_start] = np.arange(len(net.segment_start))
    j = net.osm_position[occurrence]
    way = net.osm_way[occurrence]
    origin = net.vertex_xy[j]
    unit = metres(origin[:, 1])
    reach = distance + 1.01
    side = net.osm_xy[occurrence + step] - net.osm_xy[occurrence]
    lean, q = np.full(len(j), -np.inf), np.zeros(len(j), np.int64)
    for occ, st in ((t1, s1), (t2, s2)):
        valid = occ >= 0
        o = np.where(valid, occ, 0)
        d = net.osm_xy[o + st] - net.osm_xy[o]
        dot = np.einsum("ij,ij->i", side, d) / np.maximum(np.hypot(*d.T), 1e-300)
        better = valid & (dot > lean)
        lean, q = np.where(better, dot, lean), np.where(better, net.osm_position[o] + st, q)
    # Walk each side to the next junction, or as far as it can decide a pin.
    p, along, run = j.copy(), np.zeros(len(j)), np.full(len(j), np.inf)
    walking, inside, found = np.ones(len(j), bool), np.ones(len(j), bool), []
    while walking.any():
        ahead = p + step
        walking &= (ahead >= net.way_first[way]) & (ahead <= net.way_last[way])
        ahead = np.where(walking, ahead, p)
        along += np.hypot(*(net.vertex_xy[ahead] - net.vertex_xy[p]).T) * unit
        approached[segment[np.minimum(p, ahead)[inside & walking & (along <= reach)]]] = True
        p = ahead
        meets = walking & net.vertex_junction[p]
        run[meets] = along[meets]
        inside &= walking & ~meets & (along <= reach)
        sel = np.flatnonzero(inside)
        found.append((sel, p[sel], along[sel]))
        walking &= ~meets & (along <= 2 * reach + 1)
    i, p, along = (np.concatenate(column) for column in zip(*found))
    near = 2 * along + 1 <= run[i]
    i, p = i[near], p[near]
    axis = net.vertex_xy[q[i]] - net.vertex_xy[j[i]]
    tau = np.maximum(0, np.einsum("ij,ij->i", net.vertex_xy[p] - origin[i], axis) / np.maximum(np.einsum("ij,ij->i", axis, axis), 1e-300))
    variables = np.column_stack([net.vertex_var[p], net.vertex_var[j[i]], net.vertex_var[q[i]]])
    coefficients = np.column_stack([np.ones(len(i)), -(1 - tau), -tau])
    return variables, coefficients, np.zeros(len(variables)), approached


def chain_rows(net, members):
    """(variables, coefficients, values) of z_v − (1 − τ)·z_A − τ·z_B per vertex inside a chain of the ways
    that `members` marks, with A and B the chain's ends and τ the vertex's share of its length: = 0 holds a
    tunnel straight, ≥ 0 keeps a bridge from sagging. Where the ends of member ways meet, a chain runs on
    through the two that continue most nearly straight, and the others are side tunnels. So a side tunnel or
    a shaft follows the tunnel that it joins and never lifts it: nothing but its ends gives a long tunnel its
    heights."""
    chained = np.flatnonzero(members)
    ends = {}
    for w in chained.tolist():
        for end, v, u in ((0, net.way_first[w], net.way_first[w] + 1), (1, net.way_last[w], net.way_last[w] - 1)):
            d = net.vertex_xy[u] - net.vertex_xy[v]
            ends.setdefault(int(net.vertex_var[v]), []).append((w, end, d / max(np.hypot(*d), 1e-300)))
    # At each meeting of ends, pair the ends that run most nearly straight on, the straightest pair first.
    partner = {}
    for meeting in ends.values():
        pairs = sorted((float(p[2] @ q[2]), i, k) for i, p in enumerate(meeting) for k, q in enumerate(meeting) if i < k and p[0] != q[0])
        for _, i, k in pairs:
            p, q = meeting[i][:2], meeting[k][:2]
            if p not in partner and q not in partner:
                partner[p], partner[q] = q, p
    seen, rows = set(), []
    for start in chained.tolist():
        if start in seen:
            continue
        # Back to the first way of the chain, then along it.
        w, end, steps = start, 0, 0
        while (w, end) in partner and partner[(w, end)][0] != start and steps < len(chained):
            w, end = partner[(w, end)][0], 1 - partner[(w, end)][1]
            steps += 1
        chain, flipped = [], end == 1
        while w not in seen:
            seen.add(w)
            span = np.arange(net.way_first[w], net.way_last[w] + 1)
            chain.extend((span[::-1] if flipped else span)[1 if chain else 0:].tolist())
            following = partner.get((w, 0 if flipped else 1))
            if not following:
                break
            w, flipped = following[0], following[1] == 1
        a, b = net.vertex_var[chain[0]], net.vertex_var[chain[-1]]
        if len(chain) < 3 or a == b:
            continue
        xy = net.vertex_xy[chain]
        along = np.concatenate([[0], np.cumsum(np.hypot(*np.diff(xy, axis=0).T) * metres((xy[:-1, 1] + xy[1:, 1]) / 2))])
        tau = along[1:-1] / along[-1]
        rows += [((net.vertex_var[v], a, b), (1.0, -(1 - t), -t)) for v, t in zip(chain[1:-1], tau.tolist())]
    if not rows:
        return np.zeros((0, 3), np.int64), np.zeros((0, 3)), np.zeros(0)
    return np.array([r[0] for r in rows], np.int64), np.array([r[1] for r in rows]), np.zeros(len(rows))


def parallel_pins(net, classes, terrain):
    """(variables, coefficients, values) of z_P − (1 − τ)·z_A − τ·z_B = 0 for each vertex P of a track on the
    ground and the nearest segment A-B of a track on the ground with a higher way number within PARALLEL_M
    beside it that runs its way, τ the foot of P on A-B, where the terrain at P and at its foot is known and
    differs by no more than PARALLEL_RISE_M, here and within PARALLEL_FREE_M along P's way. Each track follows only tracks after it, so no two
    rows pull against each other, and the tracks of a double line or a station lie at one height."""
    none = (np.zeros((0, 3), np.int64), np.zeros((0, 3)), np.zeros(0))
    rail = np.array([name in RAILS for name in classes])[net.rule] & ~net.free
    segments = np.flatnonzero(rail[net.segment_way])
    if not len(segments):
        return none
    start = net.segment_start[segments]
    pa, pb = net.vertex_xy[start], net.vertex_xy[start + 1]
    unit = metres(pa[:, 1])
    d = pb - pa
    length = np.hypot(*d.T)
    # One scale for the search, a little wider than the metres of any of its rows.
    scale = float(unit.max())
    tree = cKDTree((pa + pb) / 2 * scale)
    reach = (PARALLEL_M + VERTEX_M / 2) * scale / float(unit.min())
    found = []
    for w in np.flatnonzero(rail).tolist():
        span = np.arange(net.way_first[w], net.way_last[w] + 1)
        for v, near in zip(span.tolist(), tree.query_ball_point(net.vertex_xy[span] * scale, reach)):
            p = net.vertex_xy[v]
            u = v + 1 if v < net.way_last[w] else v - 1
            own = (net.vertex_xy[u] - p) / max(np.hypot(*(net.vertex_xy[u] - p)), 1e-300)
            best = None
            for k in near:
                other = net.segment_way[segments[k]]
                if other <= w or abs(float(own @ d[k]) / max(length[k], 1e-300)) < PARALLEL_COS:
                    continue
                t = float(np.clip((p - pa[k]) @ d[k] / max(length[k] ** 2, 1e-300), 0, 1))
                gap = float(np.hypot(*(p - pa[k] - t * d[k]))) * unit[k]
                if gap <= PARALLEL_M and (best is None or gap < best[0]):
                    best = (gap, k, t)
            if best:
                found.append((v, best[1], best[2]))
    if not found:
        return none
    v, k, t = (np.array(column) for column in zip(*found))
    p, foot = net.vertex_xy[v], pa[k] + t[:, None] * d[k]
    with np.errstate(invalid="ignore"):
        level = np.abs(terrain.heights(p[:, 0], p[:, 1]) - terrain.heights(foot[:, 0], foot[:, 1])) <= PARALLEL_RISE_M
    # Metres along its way at each vertex, and the pins of each way that lie too near a parting.
    steps = np.zeros(len(net.vertex_xy))
    steps[net.segment_start + 1] = net.length
    along = np.cumsum(steps)
    way = np.repeat(np.arange(len(net.ways)), net.way_last - net.way_first + 1)
    along -= along[net.way_first][way]
    for w in np.unique(way[v[~level]]).tolist():
        mine = way[v] == w
        parted = along[v[mine & ~level]]
        level[mine] &= np.min(np.abs(along[v[mine]][:, None] - parted[None, :]), axis=1) > PARALLEL_FREE_M
    v, k, t = v[level], k[level], t[level]
    variables = np.column_stack([net.vertex_var[v], net.a[segments[k]], net.b[segments[k]]]).astype(np.int64)
    return variables, np.column_stack([np.ones(len(t)), -(1 - t), -t]), np.zeros(len(t))


def ford_pins(net, rivers):
    """(variables, coefficients, values) of the fords, in rows of three variables: a road vertex that a
    waterway shares, and the point where a way tagged as a ford crosses a waterway, lie FORD_M below
    the water there. A bridge or a tunnel has no ford."""
    none = (np.zeros((0, 3), np.int64), np.zeros((0, 3)), np.zeros(0))
    if not rivers:
        return none
    levels = {}
    segments = []
    for points, z in rivers:
        for point, level in zip(points, z):
            if level is not None and math.isfinite(level):
                levels.setdefault(complex(*point), level)
        for (p, zp), (q, zq) in zip(zip(points, z), zip(points[1:], z[1:])):
            if zp is not None and zq is not None:
                segments.append((p, q, zp, zq))
    variables, coefficients, values = [], [], []
    road = ~net.free[net.osm_way]
    shared = np.flatnonzero(road & np.isin(key(net.osm_xy), np.fromiter(levels, complex, len(levels))))
    for occurrence in shared.tolist():
        variables.append([net.osm_var[occurrence]] * 3)
        coefficients.append([1.0, 0.0, 0.0])
        values.append(levels[complex(*net.osm_xy[occurrence])] - FORD_M)
    fords = [w for w, way in enumerate(net.ways) if way[5] and not way[4]]
    if fords:
        grid = {}
        for number, (p, q, _, _) in enumerate(segments):
            for cell in cells(p, q):
                grid.setdefault(cell, []).append(number)
        for w in fords:
            first, last = net.way_first[w], net.way_last[w]
            for v in range(first, last):
                p, q = net.vertex_xy[v], net.vertex_xy[v + 1]
                for number in sorted({n for cell in cells(p, q) for n in grid.get(cell, [])}):
                    r0, r1, z0, z1 = segments[number]
                    hit = crossing(p, q, np.asarray(r0), np.asarray(r1))
                    if hit is not None:
                        u, tr = hit
                        variables.append([net.vertex_var[v], net.vertex_var[v + 1], net.vertex_var[v]])
                        coefficients.append([1 - u, u, 0.0])
                        values.append(z0 + (z1 - z0) * tr - FORD_M)
    if not values:
        return none
    return np.array(variables, np.int64), np.array(coefficients), np.array(values)


def clearances(net, rivers, classes, terrain):
    """(variables, coefficients, values, ways) of the decks over what they cross, in rows of four variables: where
    a bridge segment crosses a waterway, z ≥ water + CLEAR_WATER_M + its deck; where it crosses a road, a path or a track on the ground that it does not meet, z − z_below ≥
    CLEAR_ROAD_M, CLEAR_PATH_M or CLEAR_RAIL_M + its deck; but a bridge whose end lies on that road, within its half-width,
    meets it there and has no row. Where a bridge lands at a height that it cannot move (`landings`), its deck
    climbs no steeper than it runs: a crossing that asks more lies where the bridge lands and has no row."""
    path = list(classes).index("path")
    rail = np.array([name in RAILS for name in classes])
    deck = lambda w: DECK_PATH_M if net.rule[w] >= path else DECK_M
    rows = []
    bridges = np.flatnonzero(net.free & ~net.tunnel).tolist()
    # Waterway segments with a level at both ends, and the segments on the ground in the cells of the bridges.
    below = [(p, q, (zp, zq), None) for points, z in rivers for (p, zp), (q, zq) in zip(zip(points, z), zip(points[1:], z[1:])) if zp is not None and zq is not None]
    below += [(p, q, None, road) for p, q, road in near(net, bridges, ~net.free)]
    grid = gridded(below)
    landed = landings(net)
    continued = np.bincount(net.osm_var[(net.free & ~net.tunnel)[net.osm_way]], minlength=net.osm_count) >= 2
    for w in bridges:
        first, last = net.way_first[w], net.way_last[w]
        along = np.concatenate([[0], np.cumsum(net.length[first - w:last - w])])
        tips = [along[v - first] for v in (first, last) if not continued[net.vertex_var[v]]]
        ends = [(along[v - first], float(terrain.heights(*net.vertex_xy[v, :, None])[0])) for v in (first, last) if net.vertex_var[v] in landed]

        def add(variables, coefficients, value, top, at):
            if all(top - ground <= abs(at - place) for place, ground in ends):
                rows.append((variables, coefficients, value, w))

        for v in range(first, last):
            p, q = net.vertex_xy[v], net.vertex_xy[v + 1]
            for number in sorted({k for cell in cells(p, q) for k in grid.get(cell, [])}):
                r0, r1, level, road = below[number]
                hit = crossing(p, q, np.asarray(r0), np.asarray(r1))
                if hit is None:
                    continue
                u, tr = hit
                at = along[v - first] + u * (along[v - first + 1] - along[v - first])
                if road is None:
                    value = level[0] + (level[1] - level[0]) * tr + CLEAR_WATER_M + deck(w)
                    add([net.vertex_var[v], net.vertex_var[v + 1], net.vertex_var[v], net.vertex_var[v]], [1 - u, u, 0.0, 0.0], value, value, at)
                    continue
                other, k = road
                if ({net.vertex_var[v], net.vertex_var[v + 1]} & {net.vertex_var[k], net.vertex_var[k + 1]} or not (0 <= tr < 1 and 0 <= u < 1)
                        or any(abs(at - tip) <= net.half[other] for tip in tips)):
                    continue
                clear = (CLEAR_RAIL_M if rail[net.rule[other]] else CLEAR_PATH_M if net.rule[other] >= path else CLEAR_ROAD_M) + deck(w)
                add([net.vertex_var[v], net.vertex_var[v + 1], net.vertex_var[k], net.vertex_var[k + 1]], [1 - u, u, -(1 - tr), -tr], clear,
                    float(terrain.heights(*(p + u * (q - p))[:, None])[0]) + clear, at)
    if not rows:
        return np.zeros((0, 4), np.int64), np.zeros((0, 4)), np.zeros(0), np.zeros(0, np.int64)
    return tuple(np.array([row[i] for row in rows], dtype) for i, dtype in enumerate((np.int64, np.float64, np.float64, np.int64)))


def landings(net):
    """The variables where a bridge lands at a height that it cannot move: an end that meets no other way
    stands on the terrain, and one that leaves a junction as a side lies on its through road."""
    bridge = net.free & ~net.tunnel
    last = np.append(net.osm_first[1:], len(net.osm_var)) - 1
    ends = np.concatenate([net.osm_first[bridge], last[bridge]])
    loose = ends[~net.junction[net.osm_var[ends]]]
    sides = net.sides[0][np.isin(net.sides[0], ends)]
    return set(net.osm_var[np.concatenate([loose, sides])].tolist())


def passes(net, classes):
    """The rows of the ways that cross without meeting, one over the other: a tunnel under the way that it
    crosses, by its bore and COVER_M; of two bridges that no bridge joins or that both tag different layers,
    and of two ways on the ground of different layers that are not `indoor`, the one of the higher layer, at
    an equal layer of the better class, and of one class the later one, over the other, by the clearance of
    the lower and the deck of the upper. A road on the ground over a tunnel keeps its height,
    and the tunnel keeps the margin under its centreline and both edges; any other pair moves apart, the
    stiffer less. (1 − u)·z_a + u·z_b ≷ (1 − r)·z_c + r·z_d ± the margin. Per row: the variables and
    coefficients of the upper way, of the lower, the margin, the lower way, whether the upper moves too, and
    whether the lower is a tunnel under a road on the ground."""
    path = list(classes).index("path")
    rail = np.array([name in RAILS for name in classes])
    rows = []
    grounded = np.zeros(net.count, bool)
    grounded[np.concatenate([net.a, net.b])[np.tile(~net.free[net.segment_way], 2)]] = True

    def add(upper, v, lower, k, margin):
        p, q = net.vertex_xy[v], net.vertex_xy[v + 1]
        hit = crossing(p, q, net.vertex_xy[k], net.vertex_xy[k + 1])
        if hit is None or {net.vertex_var[v], net.vertex_var[v + 1]} & {net.vertex_var[k], net.vertex_var[k + 1]}:
            return
        u, r = hit
        if not (0 <= u < 1 and 0 <= r < 1):
            return
        above = ((net.vertex_var[v], net.vertex_var[v + 1]), (1 - u, u))
        under = (net.vertex_var[k], net.vertex_var[k + 1])
        if not (net.tunnel[lower] and not net.free[upper]):
            rows.append((*above, under, (1 - r, r), margin, lower, True, False))
            return
        # Where the tunnel's line meets each edge of the road, on this segment of the tunnel at most.
        e = q - p
        side = np.array([-e[1], e[0]]) / np.hypot(*e) * net.half[upper] / metres(p[1])
        places = [r]
        for offset in (side, -side):
            if (edge := lines(net.vertex_xy[k], net.vertex_xy[k + 1], p + offset, e)) is None:
                continue
            x = min(max(edge[0], 0.0), 1.0)
            # An edge past a portal would pin the portal, and with it the way on the ground that leaves it.
            if x != edge[0] and grounded[under[int(x)]]:
                continue
            places.append(x)
        rows.extend((*above, under, (1 - x, x), margin, lower, False, True) for x in places)

    def crossed(ways, among):
        grid = gridded(segments := near(net, ways, among))
        for w in ways:
            for v in range(net.way_first[w], net.way_last[w]):
                for number in sorted({i for cell in cells(net.vertex_xy[v], net.vertex_xy[v + 1]) for i in grid.get(cell, [])}):
                    yield w, v, *segments[number][2]

    for w, v, other, k in crossed(np.flatnonzero(net.tunnel).tolist(), ~net.tunnel):
        add(other, k, w, v, (BORE_RAIL_M if rail[net.rule[w]] else BORE_PATH_M if net.rule[w] >= path else BORE_M) + COVER_M)
    bridge = net.free & ~net.tunnel
    # The bridges that bridge ways join are one structure, such as a spiral stair: its ways set their heights,
    # but where both tag different layers, as the ramps of an interchange. Of two of one layer and class, the
    # later one passes over.
    on = bridge[net.osm_way]
    links = sparse.csr_matrix((np.ones(np.count_nonzero(on)), (net.osm_way[on], net.osm_var[on])), shape=(len(net.ways), net.osm_count))
    _, structure = connected_components(sparse.bmat([[None, links], [links.T, None]]), directed=False)
    margin = lambda upper, lower: (CLEAR_RAIL_M if rail[net.rule[lower]] else CLEAR_PATH_M if net.rule[lower] >= path else CLEAR_ROAD_M) + (DECK_PATH_M if net.rule[upper] >= path else DECK_M)
    rank = lambda i: (net.layer[i], -net.rule[i], i)
    for w, v, other, k in crossed(np.flatnonzero(bridge).tolist(), bridge):
        if rank(w) > rank(other) and (structure[w] != structure[other] or (net.layered[w] and net.layered[other] and net.layer[w] != net.layer[other])):
            add(w, v, other, k, margin(w, other))
    # A pair of ways on the ground of different layers has one of layer 0 at most; the other one finds it.
    ground = ~net.free & ~net.indoor
    for w, v, other, k in crossed(np.flatnonzero(ground & (net.layer != 0)).tolist(), ground):
        if net.layer[w] > net.layer[other]:
            add(w, v, other, k, margin(w, other))
        elif net.layer[w] < net.layer[other] and net.layer[other] == 0:
            add(other, k, w, v, margin(other, w))
    if not rows:
        return np.zeros((0, 2), np.int64), np.zeros((0, 2)), np.zeros((0, 2), np.int64), np.zeros((0, 2)), np.zeros(0), np.zeros(0, np.int64), np.zeros(0, bool), np.zeros(0, bool)
    return tuple(np.array([row[i] for row in rows]) for i in range(8))


def near(net, ways, among):
    """The segments of the ways that `among` marks in the grid cells of `ways`: (start, end, (way, its first vertex))."""
    cells_of_ways = {cell for w in ways for v in range(net.way_first[w], net.way_last[w]) for cell in cells(net.vertex_xy[v], net.vertex_xy[v + 1])}
    ground = np.flatnonzero(among[net.segment_way])
    start = net.segment_start[ground]
    lo = np.floor(np.minimum(net.vertex_xy[start], net.vertex_xy[start + 1]) / GRID_UNITS).astype(np.int64)
    hi = np.floor(np.maximum(net.vertex_xy[start], net.vertex_xy[start + 1]) / GRID_UNITS).astype(np.int64)
    # A segment is shorter than a cell: its corner cells hold it.
    corners = [(lo[:, 0], lo[:, 1]), (hi[:, 0], lo[:, 1]), (lo[:, 0], hi[:, 1]), (hi[:, 0], hi[:, 1])]
    hit = np.zeros(len(start), bool)
    keys = np.array(sorted(cx * (1 << 32) + cy for cx, cy in cells_of_ways), np.int64)
    for cx, cy in corners:
        hit |= np.isin(cx * (1 << 32) + cy, keys)
    return [(net.vertex_xy[v], net.vertex_xy[v + 1], (int(net.segment_way[i]), v)) for i in ground[hit].tolist() for v in [int(net.segment_start[i])]]


def gridded(segments):
    """Per cell of the grid, the numbers of the segments (start, end, ...) whose boxes meet it."""
    grid = {}
    for number, (p, q, *_) in enumerate(segments):
        for cell in cells(p, q):
            grid.setdefault(cell, []).append(number)
    return grid


def cells(p, q):
    """The cells of the waterway grid that the box of segment p-q meets."""
    return [(cx, cy) for cx in range(int(min(p[0], q[0]) // GRID_UNITS), int(max(p[0], q[0]) // GRID_UNITS) + 1)
            for cy in range(int(min(p[1], q[1]) // GRID_UNITS), int(max(p[1], q[1]) // GRID_UNITS) + 1)]


def lines(p, q, r, e):
    """The parameters (along p-q, along e from r) where the lines through p-q and along e from r meet, or None."""
    d = q - p
    denominator = d[0] * e[1] - d[1] * e[0]
    if denominator == 0:
        return None
    f = r - p
    return (f[0] * e[1] - f[1] * e[0]) / denominator, (f[0] * d[1] - f[1] * d[0]) / denominator


def crossing(p, q, r, s):
    """The parameters (along p-q, along r-s) where two segments cross, or None."""
    hit = lines(p, q, r, s - r)
    return hit if hit is not None and 0 <= hit[0] <= 1 and 0 <= hit[1] <= 1 else None


def kept(net, z, known):
    """Per vertex, whether the tiles keep it: each vertex of OSM and at an approach distance, and an added
    vertex where the profile leaves the straight line between its neighbours by more than KEEP_M
    (Douglas-Peucker in the plane of distance and height, over each OSM segment)."""
    keep = net.vertex_fixed.copy()
    added = np.flatnonzero(~net.vertex_osm)
    if not len(added):
        return keep
    segment = net.vertex_segment[added]
    starts = np.flatnonzero(np.concatenate([[True], segment[1:] != segment[:-1]]))
    # The OSM vertices about the added ones of each segment.
    first = added[starts] - 1
    last = np.concatenate([added[starts[1:] - 1], [added[-1]]]) + 1
    zv = np.where(known[net.vertex_var], z[net.vertex_var], np.nan)
    span = np.repeat(np.arange(len(starts)), np.diff(np.concatenate([starts, [len(added)]])))
    line = zv[first[span]] + (zv[last[span]] - zv[first[span]]) * net.vertex_t[added]
    off = np.abs(zv[added] - line)
    off = np.where(np.isfinite(off), off, np.inf)
    worst = np.maximum.reduceat(off, starts)
    for s in np.flatnonzero(worst > KEEP_M).tolist():
        i, j = int(first[s]), int(last[s])
        if not np.isfinite(zv[i:j + 1]).all():
            keep[i:j + 1] = True
            continue
        stops = [i, *(i + 1 + np.flatnonzero(net.vertex_fixed[i + 1:j])).tolist(), j]
        spans = list(zip(stops, stops[1:]))
        while spans:
            p, q = spans.pop()
            if q - p < 2:
                continue
            inner = np.arange(p + 1, q)
            tp, tq = net.vertex_t[p] if p != i else 0.0, net.vertex_t[q] if q != j else 1.0
            ti = net.vertex_t[inner]
            line = zv[p] + (zv[q] - zv[p]) * (ti - tp) / (tq - tp)
            far = int(np.argmax(np.abs(zv[inner] - line)))
            if abs(zv[inner][far] - line[far]) > KEEP_M:
                keep[inner[far]] = True
                spans += [(p, int(inner[far])), (int(inner[far]), q)]
    return keep


def culverts_of(net, z, seg, t, h, finite):
    """Per way, whether it is a culvert: a bridge shorter than CULVERT_M, with the deck no more than
    CULVERT_RISE_M above the terrain under it."""
    way_length = np.bincount(net.segment_way, net.length, len(net.ways))
    bridge = net.free & ~net.tunnel & (way_length < CULVERT_M)
    sample = np.flatnonzero(bridge[net.segment_way[seg]] & finite)
    rise = np.full(len(net.ways), -np.inf)
    deck = z[net.a[seg[sample]]] * (1 - t[sample]) + z[net.b[seg[sample]]] * t[sample]
    np.maximum.at(rise, net.segment_way[seg[sample]], deck - h[sample])
    return bridge & (rise > -np.inf) & (rise <= CULVERT_RISE_M)
