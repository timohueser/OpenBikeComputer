"""The road solver of the TrailView feature tiles: a height on each vertex of the roads, paths, tunnels and
galleries, from the master terrain, by the rules per `highway` class of `data/trailview/roads.json`.

A way is sampled every SAMPLE_M metres. Its heights minimise Σ wᵢ(zᵢ − hᵢ)² + λ·Σ Δgrade² / s, with Huber
weights wᵢ, so that a spike or a gully of the terrain does not move the road, and λ = smooth_m⁴ of its
class. No segment is steeper than the grade of its class. Ways that share a vertex share its height, and
a side road leaves a junction on the surface of the through road. A bridge or a tunnel ignores the terrain
under it and runs between its ends, a tunnel in a straight line; a bridge never sags under the straight line
between its ends, and rises where it would pass too low over the river, the lake, the road or the path that it
crosses; a short bridge that lies on the terrain is a culvert and does not. A tunnel sinks under a road or a path
that crosses over it, which does not move. Of two bridges that cross, the one of the higher layer passes over the
other, at one layer the better class. A ford lies FORD_M below the water it crosses."""

import json
import math
from pathlib import Path

import numpy as np
from scipy import sparse
from scipy.sparse.csgraph import connected_components, dijkstra
from scipy.sparse.linalg import splu

from .profiles import metres
from .structures import BORE_M, BORE_PATH_M
from .water import contains

TABLE = Path(__file__).resolve().parents[2] / "data/trailview/roads.json"
FIELDS = {"grade", "smooth_m", "tol", "reach", "least", "cut", "fill", "tilt"}
KINDS = {"main-road", "road", "path", "tunnel", "gallery"}
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
# The least height of a deck's road over the water, the road and the path that it crosses: the clearance
# under the deck and the deck's depth (DECK_M for a road, DECK_PATH_M for a path).
CLEAR_WATER_M, CLEAR_ROAD_M, CLEAR_PATH_M = 1.5, 4.5, 2.5
DECK_M, DECK_PATH_M = 1.0, 0.4
# A road or a path over a tunnel lies at least this far over its bore.
COVER_M = 1.0
# Weights against one metre of terrain samples: the terrain under a bridge or tunnel that meets no road,
# a row that pins a junction approach, a ford or a slope limit, and per metre of a bridge or tunnel the
# preference for level, so that one that meets a road at one end only is level from there.
FREE, PIN, LEVEL = 1e-4, 1e8, 1e-3
# Solves of the whole network, the first ones with the Huber weights of the solve before. Then rounds
# of the slope limits solve only the network within REGION_M of the segments whose limit changed, and
# the bridges and tunnels it meets: the terrain anchors the rest.
SOLVES, ROUNDS, REGION_M = 2, 40, 300.0
# The grade excess in metres that a segment may have before it holds its limit.
TOLERANCE_M = 1e-3
# Terrain samples per call, and the cell of the grid of waterways in world units (about 100 m).
CHUNK, GRID_UNITS = 1 << 21, 2.5e-6


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


def rule_of(kind, tags, classes=CLASSES):
    """The row of the class table of a way: its class, else that of its kind."""
    name = tags.get("highway", "")
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
            free = kind == "tunnel" or tags.get("bridge", "no") != "no"
            for p, (_, points, _) in enumerate(parts):
                xy = np.asarray(points, np.float64).reshape(-1, 2)
                # A repeated vertex has no length to solve along.
                xy = xy[np.concatenate([[True], np.any(np.diff(xy, axis=0) != 0, axis=1)])] if len(xy) else xy
                if len(xy) >= 2:
                    self.ways.append((f, p, xy, rule, free, tags.get("ford", "no") != "no"))
        if not self.ways:
            return
        counts = np.array([len(way[2]) for way in self.ways])
        self.rule = np.array([way[3] for way in self.ways])
        self.free = np.array([way[4] for way in self.ways], bool)
        self.tunnel = np.array([features[way[0]][0] == "tunnel" for way in self.ways], bool)
        self.layer = np.array([layer(features[way[0]][1]) for way in self.ways])
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


def solve(features, terrain, rivers=(), classes=CLASSES, lakes=()):
    """Per feature (kind, tags, parts): its parts (outer, points, heights) with the heights solved, None
    where the terrain is unknown, and vertices added where the profile bends between two of OSM; the
    features that are culverts; the tunnels that pass under a road or a path; and the metrics of the solve. `rivers` lists (points, levels) of the
    solved waterways, for the fords and the decks, and `lakes` (rings, level) of the lakes, for the decks."""
    out = [[(outer, points, None) for outer, points, _ in parts] for _, _, parts in features]
    net = Network(features, classes)
    if not net.ways:
        return out, set(), set(), {}
    n, a, b, length = net.count, net.a, net.b, net.length
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
    # A bridge or a tunnel sees no terrain, unless nothing that it meets does.
    weight = (length / k)[seg] * ~free[seg]
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
    above = clearances(net, rivers, lakes, classes, terrain)
    sags = chain_rows(net, net.free & ~net.tunnel)
    above = (np.concatenate([above[0], np.column_stack([sags[0], sags[0][:, 0]])]), np.concatenate([above[1], np.pad(sags[1], ((0, 0), (0, 1)))]),
             np.concatenate([above[2], sags[2]]), np.concatenate([above[3], np.full(len(sags[2]), -1)]))
    whole = np.all(known[above[0]], axis=1)
    C = sparse.csr_matrix((above[1].ravel(), (np.repeat(np.arange(len(above[2])), 4), above[0].ravel())), shape=(len(above[2]), n))[whole]
    least, deck_way = above[2][whole], above[3][whole]
    clear = deck_way >= 0
    under = passes(net, classes)
    held_up = np.all(known[under[0]], axis=1) & np.all(known[under[2]], axis=1)
    pairs = lambda variables, coefficients: sparse.csr_matrix((coefficients.ravel(), (np.repeat(np.arange(len(variables)), 2), variables.ravel())), shape=(len(variables), n))
    T, R, need, sign = pairs(under[0][held_up], under[1][held_up]), pairs(under[2][held_up], under[3][held_up]), under[4][held_up], under[6][held_up]
    # A row of two bridges holds them apart; a tunnel's row holds the tunnel to a height under the road.
    both = sign > 0
    apart = T - sparse.diags(both.astype(float)) @ R
    variables = np.concatenate([approach[0], fords[0], bores[0]])
    coefficients = np.concatenate([approach[1], fords[1], bores[1]])
    values = np.concatenate([approach[2], fords[2], bores[2]])
    ok = np.all(known[variables], axis=1)
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
    fixed = (D2.T @ sparse.diags(lam) @ D2 + PIN * (P.T @ P) + L1.T @ sparse.diags(LEVEL / np.maximum(length[level], 0.01)) @ L1
             + sparse.diags(np.where(known, 0.0, 1.0)))
    pinned = PIN * (P.T @ values)

    def factor(matrix):
        return splu(matrix.tocsc(), permc_spec="MMD_AT_PLUS_A", diag_pivot_thresh=0.0, options={"SymmetricMode": True})

    def update(held, gz, release=True):
        """The slope limits after a solve: a segment past its limit holds it, a held one that the solve
        would make less steep lets go. Without releases the rounds end: the held segments only grow."""
        out = held.copy()
        if release:
            out[held * gz < bound] = 0
        out[gz > bound + TOLERANCE_M] = 1
        out[gz < -bound - TOLERANCE_M] = -1
        return out

    held = np.zeros(len(limited))
    w = weight
    for solves_ in range(SOLVES):
        ca, cb = 1 - t, t
        aa, bb, ab = (np.bincount(seg, w * c, m) for c in (ca * ca, cb * cb, ca * cb))
        base = sparse.csr_matrix((np.concatenate([aa, bb, ab, ab]), (np.concatenate([a, b, a, b]), np.concatenate([a, b, b, a]))), shape=(n, n)) + fixed
        rhs = np.bincount(a, np.bincount(seg, w * ca * h, m), n) + np.bincount(b, np.bincount(seg, w * cb * h, m), n) + pinned
        on = np.flatnonzero(held)
        limits = G[on]
        z = factor(base + PIN * (limits.T @ limits)).solve(rhs + PIN * (limits.T @ (held[on] * bound[on])))
        if solves_ < SOLVES - 1:
            residual = np.abs(z[a[seg]] * (1 - t) + z[b[seg]] * t - h)
            w = weight * np.minimum(1.0, HUBER_M / np.maximum(residual, 1e-9))
            # A short bridge that lies on the terrain is a culvert. Any other deck that passes too low holds
            # its least height from the next solve on.
            culvert = culverts_of(net, z, seg, t, h, finite)
            low = (C @ z < least - TOLERANCE_M) & ~(clear & culvert[deck_way])
            fixed = fixed + PIN * (C[low].T @ C[low])
            pinned = pinned + PIN * (C[low].T @ least[low])
            # A way that passes too near over or under another holds the margin from the next solve on.
            near = sign * (T @ z - R @ z) < need - TOLERANCE_M
            target = np.where(both, 0, R @ z) + sign * need
            fixed = fixed + PIN * (apart[near].T @ apart[near])
            pinned = pinned + PIN * (apart[near].T @ target[near])
        before, held = held, update(held, G @ z)
    # Rounds of the slope limits near the segments whose limit changed, the rest of the network fixed.
    # Nothing anchors a bridge or a tunnel but its ends: a region takes it whole.
    graph = sparse.csr_matrix((np.where(free, 1e-6, np.maximum(length, 0.01)), (a, b)), shape=(n, n))
    graph = graph + graph.T
    rounds = 0
    base = base.tocsr()
    while rounds < ROUNDS and np.any(changed := held != before):
        rounds += 1
        sources = np.unique(np.concatenate([a[limited[changed]], b[limited[changed]]]))
        region = np.flatnonzero(np.isfinite(dijkstra(graph, indices=sources, limit=REGION_M, min_only=True)))
        on = np.flatnonzero(held)
        limits = G[on]
        local = limits[:, region]
        residual = (rhs[region] + PIN * (local.T @ (held[on] * bound[on])) - base[region] @ z - PIN * (local.T @ (limits @ z)))
        z[region] += factor(base[region][:, region] + PIN * (local.T @ local)).solve(residual)
        before, held = held, update(held, G @ z, rounds < ROUNDS // 2)

    keep = kept(net, z, known)
    for w_, (f, p, *_) in enumerate(net.ways):
        span = slice(net.way_first[w_], net.way_last[w_] + 1)
        mask = keep[span]
        points = list(map(tuple, net.vertex_xy[span][mask].tolist()))
        v = net.vertex_var[span][mask]
        heights = np.where(known[v], z[v], np.nan).tolist()
        out[f][p] = (out[f][p][0], points, [None if math.isnan(value) else value for value in heights])
    culverts = {net.ways[w][0] for w in np.flatnonzero(culvert).tolist()}
    excess = np.abs(G @ z) - bound
    worst = limited[np.argmax(excess)] if len(excess) else 0
    lon, lat = world_degrees(net.vertex_xy[net.segment_start[worst]]) if len(excess) else (0, 0)
    residual = np.abs(z[a[seg]] * (1 - t) + z[b[seg]] * t - h)
    metrics = {
        "ways": len(net.ways), "variables": n, "known": int(known.sum()), "approaches": int(np.count_nonzero(ok[:len(approach[2])])),
        "fords": int(np.count_nonzero(ok[len(approach[2]):len(approach[2]) + len(fords[2])])), "culverts": len(culverts), "rounds": rounds, "held": int(np.count_nonzero(held)),
        "clearances": int(np.count_nonzero(clear & ~culvert[deck_way])),
        "clearances_low_cm": int(np.count_nonzero((C @ z < least - 0.01) & clear & ~culvert[deck_way])),
        "passes": len(need), "passes_near_cm": int(np.count_nonzero(sign * (T @ z - R @ z) < need - 0.01)),
        "grade_excess_m": round(float(np.max(excess, initial=0)), 4), "grade_excess_at": [round(lat, 5), round(lon, 5)],
        "grade_excess_over_1cm": int(np.count_nonzero(excess > 0.01)),
        "residual_p99_m": round(float(np.percentile(residual[~free[seg] & (weight > 0)], 99)), 2) if (~free[seg] & (weight > 0)).any() else 0,
    }
    return out, culverts, {net.ways[w][0] for w in under[5][held_up].tolist() if net.tunnel[w]}, metrics


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


def clearances(net, rivers, lakes, classes, terrain):
    """(variables, coefficients, values, ways) of the decks over what they cross, in rows of four variables, at
    the vertices of the deck segment that crosses: over a waterway z ≥ water + CLEAR_WATER_M + its deck, and so
    at each bridge vertex over a lake; over a road or a path on the ground that it does not meet,
    z − z_below ≥ CLEAR_ROAD_M or CLEAR_PATH_M + its deck. A row on the crossing point alone would be a lever
    on the far vertex. Where a bridge lands at a height that it cannot move (`landings`), its deck climbs no
    steeper than it runs: a crossing that asks more lies where the bridge lands and has no row."""
    path = list(classes).index("path")
    deck = lambda w: DECK_PATH_M if net.rule[w] >= path else DECK_M
    rows = []
    bridges = np.flatnonzero(net.free & ~net.tunnel).tolist()
    # Waterway segments with a level at both ends, and the segments on the ground in the cells of the bridges.
    below = [(p, q, (zp, zq), None) for points, z in rivers for (p, zp), (q, zq) in zip(zip(points, z), zip(points[1:], z[1:])) if zp is not None and zq is not None]
    below += [(p, q, None, road) for p, q, road in near(net, bridges, ~net.free)]
    grid = gridded(below)
    boxes = np.array([(*np.min([np.min(ring, 0) for ring in rings], 0), *np.max([np.max(ring, 0) for ring in rings], 0)) for rings, _ in lakes]).reshape(-1, 4)
    landed = landings(net)
    for w in bridges:
        first, last = net.way_first[w], net.way_last[w]
        along = np.concatenate([[0], np.cumsum(net.length[first - w:last - w])])
        ends = [(along[v - first], float(terrain.heights(*net.vertex_xy[v, :, None])[0])) for v in (first, last) if net.vertex_var[v] in landed]

        def add(vertices, others, coefficients, value, top, at):
            if all(top - ground <= abs(at - place) for place, ground in ends):
                rows.extend(([net.vertex_var[e], *others], coefficients, value, w) for e in vertices)

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
                    add((v, v + 1), [net.vertex_var[v]] * 3, [1.0, 0.0, 0.0, 0.0], value, value, at)
                    continue
                other, k = road
                if {net.vertex_var[v], net.vertex_var[v + 1]} & {net.vertex_var[k], net.vertex_var[k + 1]} or not (0 <= tr < 1 and 0 <= u < 1):
                    continue
                clear = (CLEAR_PATH_M if net.rule[other] >= path else CLEAR_ROAD_M) + deck(w)
                add((v, v + 1), [net.vertex_var[k], net.vertex_var[k + 1], net.vertex_var[k]], [1.0, -(1 - tr), -tr, 0.0], clear,
                    float(terrain.heights(*(p + u * (q - p))[:, None])[0]) + clear, at)
        xy = net.vertex_xy[first:last + 1]
        lo, hi = xy.min(0), xy.max(0)
        for i in np.flatnonzero((boxes[:, 0] <= hi[0]) & (boxes[:, 2] >= lo[0]) & (boxes[:, 1] <= hi[1]) & (boxes[:, 3] >= lo[1])).tolist():
            rings, level = lakes[i]
            for v in np.flatnonzero(contains(rings, xy[:, 0], xy[:, 1])).tolist():
                value = level + CLEAR_WATER_M + deck(w)
                add((first + v,), [net.vertex_var[first + v]] * 3, [1.0, 0.0, 0.0, 0.0], value, value, along[v])
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
    """The rows of the ways that pass under or over a way that they do not meet:
    (1 − u)·z_a + u·z_b ≷ (1 − r)·z_c + r·z_d ∓ the margin. A tunnel passes under a road or a path on the
    ground by its bore and COVER_M, and the road keeps its height. Of two bridges that no bridge joins, the one
    of the higher layer, at an equal layer of the better class, passes over the other by the clearance of the
    lower and the deck of the upper; both move, the stiffer less. The mover's variables and coefficients,
    the other's, the margins, the mover's way and the sign, +1 over a bridge and −1 under a road, per row."""
    path = list(classes).index("path")
    rows = []

    def add(w, v, other, k, margin, sign):
        p, q = net.vertex_xy[v], net.vertex_xy[v + 1]
        hit = crossing(p, q, net.vertex_xy[k], net.vertex_xy[k + 1])
        if hit is None or {net.vertex_var[v], net.vertex_var[v + 1]} & {net.vertex_var[k], net.vertex_var[k + 1]}:
            return
        u, r = hit
        if not (0 <= u < 1 and 0 <= r < 1):
            return
        if sign < 0:
            rows.append(((net.vertex_var[v], net.vertex_var[v + 1]), (1 - u, u), (net.vertex_var[k], net.vertex_var[k + 1]), (1 - r, r), margin, w, sign))
            return
        # Two decks keep the margin between all ends of their crossing segments: a row on the crossing point
        # would be a lever on the far ends.
        rows.extend(((net.vertex_var[a], net.vertex_var[a]), (1.0, 0.0), (net.vertex_var[b], net.vertex_var[b]), (1.0, 0.0), margin, w, sign)
                    for a in (v, v + 1) for b in (k, k + 1))

    def crossed(ways, among):
        grid = gridded(segments := near(net, ways, among))
        for w in ways:
            for v in range(net.way_first[w], net.way_last[w]):
                for number in sorted({i for cell in cells(net.vertex_xy[v], net.vertex_xy[v + 1]) for i in grid.get(cell, [])}):
                    yield w, v, *segments[number][2]

    for w, v, other, k in crossed(np.flatnonzero(net.tunnel).tolist(), ~net.free):
        add(w, v, other, k, (BORE_PATH_M if net.rule[w] >= path else BORE_M) + COVER_M, -1)
    bridge = net.free & ~net.tunnel
    # The bridges that bridge ways join are one structure, such as a spiral stair: its ways set its heights.
    on = bridge[net.osm_way]
    links = sparse.csr_matrix((np.ones(np.count_nonzero(on)), (net.osm_way[on], net.osm_var[on])), shape=(len(net.ways), net.osm_count))
    _, structure = connected_components(sparse.bmat([[None, links], [links.T, None]]), directed=False)
    for w, v, other, k in crossed(np.flatnonzero(bridge).tolist(), bridge):
        rank = lambda i: (net.layer[i], -net.rule[i])
        if rank(w) > rank(other) and structure[w] != structure[other]:
            add(w, v, other, k, (CLEAR_PATH_M if net.rule[other] >= path else CLEAR_ROAD_M) + (DECK_PATH_M if net.rule[w] >= path else DECK_M), 1)
    if not rows:
        return np.zeros((0, 2), np.int64), np.zeros((0, 2)), np.zeros((0, 2), np.int64), np.zeros((0, 2)), np.zeros(0), np.zeros(0, np.int64), np.zeros(0)
    return tuple(np.array([row[i] for row in rows]) for i in range(7))


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


def crossing(p, q, r, s):
    """The parameters (along p-q, along r-s) where two segments cross, or None."""
    d, e = q - p, s - r
    denominator = d[0] * e[1] - d[1] * e[0]
    if denominator == 0:
        return None
    f = r - p
    tp = (f[0] * e[1] - f[1] * e[0]) / denominator
    tr = (f[0] * d[1] - f[1] * d[0]) / denominator
    return (tp, tr) if 0 <= tp <= 1 and 0 <= tr <= 1 else None


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
