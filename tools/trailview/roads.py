"""The road solver of the TrailView feature tiles: a height on each vertex of the roads, paths, tunnels and
galleries, from the master terrain, by the rules per `highway` class of `data/trailview/roads.json`.

A way is sampled every SAMPLE_M metres. Its heights minimise Σ wᵢ(zᵢ − hᵢ)² + λ·Σ Δgrade² / s, with Huber
weights wᵢ, so that a spike or a gully of the terrain does not move the road, and λ = smooth_m⁴ of its
class. No segment is steeper than the grade of its class. Ways that share a vertex share its height, and
a side road leaves a junction on the surface of the through road. A bridge or a tunnel ignores the terrain
under it and runs between its ends. A ford lies FORD_M below the water it crosses."""

import json
import math
from pathlib import Path

import numpy as np
from scipy import sparse
from scipy.sparse.linalg import splu

from .profiles import metres

TABLE = Path(__file__).resolve().parents[2] / "data/trailview/roads.json"
CLASSES = json.loads(TABLE.read_text())
RANKS = {name: rank for rank, name in enumerate(CLASSES)}
KINDS = {"main-road", "road", "path", "tunnel", "gallery"}
# A cable route on rock has no bench to stamp.
UNSOLVED = {"via_ferrata"}
SAMPLE_M = 2.0
# While it solves, a way has a vertex at least every VERTEX_M; the tiles keep one only where the profile
# bends by more than KEEP_M there.
VERTEX_M, KEEP_M = 10.0, 0.05
# A side road lies on the surface of the through road for this far from their junction; a path that
# meets a path, for APPROACH_PATH_M.
APPROACH_M, APPROACH_PATH_M = 4.0, 1.5
HUBER_M = 1.0
FORD_M = 0.2
# A bridge shorter than this that lies no higher than CULVERT_RISE_M over the terrain is a road over a culvert.
CULVERT_M, CULVERT_RISE_M = 8.0, 1.0
# Weights against one metre of terrain samples: the terrain under a bridge or tunnel, and a row that
# pins a junction approach, a ford or a slope limit.
FREE, PIN = 1e-4, 1e8
# Rounds of the solve at most, the rounds that update the Huber weights, and the grade excess in metres
# that a segment may have before it holds its limit.
ROUNDS, HUBER_ROUNDS, TOLERANCE_M = 12, 3, 1e-3
# Terrain samples per call, and the cell of the grid of waterways in world units (about 100 m).
CHUNK, GRID_UNITS = 1 << 21, 2.5e-6


def solves(kind, tags):
    """Whether this module sets the heights of a feature."""
    return kind in KINDS and tags.get("highway") not in UNSOLVED


def rule_of(kind, tags):
    """The row of `roads.json` of a way: its class, else that of its kind."""
    name = tags.get("highway", "")
    return name if name in CLASSES else ("path" if kind == "path" else "road")


def key(points):
    """One complex number per point, equal for equal points."""
    return points[:, 0] + 1j * points[:, 1]


class Ways:
    """The ways of the features, cut into segments of at most VERTEX_M, with one variable per vertex: a
    vertex of OSM that ways share is one variable."""

    def __init__(self, features):
        self.ways = []
        for f, (kind, tags, parts) in enumerate(features):
            rule = rule_of(kind, tags)
            free = kind == "tunnel" or tags.get("bridge", "no") != "no"
            for p, (_, points, _) in enumerate(parts):
                xy = np.asarray(points, np.float64)
                # A repeated vertex has no length to solve along.
                xy = xy[np.concatenate([[True], np.any(np.diff(xy, axis=0) != 0, axis=1)])]
                if len(xy) >= 2:
                    self.ways.append((f, p, xy, rule, free, tags.get("ford", "no") != "no"))
        points = np.concatenate([way[2] for way in self.ways])
        keys, self.osm = np.unique(key(points), return_inverse=True)
        self.osm_points = np.column_stack([keys.real, keys.imag])
        counts = np.bincount(self.osm, minlength=len(keys))
        offsets = np.cumsum([0] + [len(way[2]) for way in self.ways])
        self.osm_of = [self.osm[offsets[w]:offsets[w + 1]] for w in range(len(self.ways))]
        self.junctions = counts >= 2
        self.approaches = self.junction_roles()
        self.densify()

    def junction_roles(self):
        """Per junction, its through road and the side directions that join it: {(way, vertex, step): (through
        directions, approach metres)}. The through road is the best class at the junction, and of two
        ways of that class, the pair that runs most nearly straight on."""
        at = {}
        for w, (_, _, points, rule, _, _) in enumerate(self.ways):
            ids = self.osm_of[w]
            for j in np.flatnonzero(self.junctions[ids]):
                for step in (1, -1):
                    if 0 <= j + step < len(points):
                        d = points[j + step] - points[j]
                        norm = math.hypot(*d)
                        if norm > 0:
                            at.setdefault(ids[j], []).append((RANKS[rule], w, j, step, d / norm))
        roles = {}
        for directions in at.values():
            best = min(rank for rank, *_ in directions)
            top = [d for d in directions if d[0] == best]
            if len(top) >= 2:
                pairs = [(float(a[4] @ b[4]), i, k) for i, a in enumerate(top) for k, b in enumerate(top) if i < k]
                _, i, k = min(pairs)
                through = [top[i], top[k]]
            else:
                through = top[:1]
            path = best >= RANKS["path"]
            for direction in directions:
                if not any(direction is t for t in through):
                    roles[direction[1:4]] = ([t[1:4] for t in through], APPROACH_PATH_M if path else APPROACH_M)
        return roles

    def densify(self):
        """The vertices of each way: its own, one at the approach distance of each side direction, and
        enough between them that no segment is longer than VERTEX_M."""
        n = len(self.osm_points)
        self.vertex, self.point, self.osm_vertex = [], [], []
        # Per way: the position of each OSM vertex among its vertices.
        self.index = []
        sides = {}
        for (w, j, step), (_, distance) in self.approaches.items():
            sides.setdefault(w, []).append((j, step, distance))
        for w, (_, _, points, _, _, _) in enumerate(self.ways):
            d = np.diff(points, axis=0)
            lengths = np.hypot(*d.T) * metres((points[:-1, 1] + points[1:, 1]) / 2)
            cuts = [list(np.arange(1, max(1, math.ceil(length / VERTEX_M))) / max(1, math.ceil(length / VERTEX_M))) for length in lengths]
            for j, step, distance in sides.get(w, []):
                s = j if step == 1 else j - 1
                if lengths[s] > distance + 1:
                    t = distance / lengths[s] if step == 1 else 1 - distance / lengths[s]
                    cuts[s] = sorted({*(c for c in cuts[s] if abs(c - t) * lengths[s] > 1), t})
            ids, xy, osm, index = [self.osm_of[w][0]], [points[0]], [True], [0]
            for s, ts in enumerate(cuts):
                for t in ts:
                    ids.append(n)
                    n += 1
                    xy.append(points[s] + t * d[s])
                    osm.append(False)
                index.append(len(ids))
                ids.append(self.osm_of[w][s + 1])
                xy.append(points[s + 1])
                osm.append(True)
            self.vertex.append(np.array(ids))
            self.point.append(np.array(xy))
            self.osm_vertex.append(np.array(osm))
            self.index.append(np.array(index))
        self.count = n


def solve(features, terrain, rivers=()):
    """Per feature (kind, tags, parts): its parts (outer, points, heights) with the heights solved, None
    where the terrain is unknown, and vertices added where the profile bends between two of OSM; the
    features that are culverts; and the metrics of the solve. `rivers` lists (points, levels) of the
    solved waterways, for the fords."""
    ways = Ways(features)
    n = ways.count
    rules = [CLASSES[way[3]] for way in ways.ways]
    # Segments, as variable pairs, with their way and length.
    a = np.concatenate([v[:-1] for v in ways.vertex])
    b = np.concatenate([v[1:] for v in ways.vertex])
    pa = np.concatenate([p[:-1] for p in ways.point])
    pb = np.concatenate([p[1:] for p in ways.point])
    seg_way = np.concatenate([np.full(len(v) - 1, w) for w, v in enumerate(ways.vertex)])
    length = np.hypot(*(pb - pa).T) * metres((pa[:, 1] + pb[:, 1]) / 2)
    free = np.array([way[4] for way in ways.ways])[seg_way]
    # Samples: k per segment from its start, and the last vertex of each way.
    k = np.maximum(1, np.ceil(length / SAMPLE_M)).astype(np.int64)
    seg = np.repeat(np.arange(len(a)), k)
    t = (np.arange(len(seg)) - np.repeat(np.cumsum(k) - k, k)) / k[seg]
    last = np.cumsum([len(v) - 1 for v in ways.vertex]) - 1
    seg, t = np.concatenate([seg, last]), np.concatenate([t, np.ones(len(last))])
    weight = (length / k)[seg] * np.where(free[seg], FREE, 1.0)
    h = np.empty(len(seg), np.float64)
    for start in range(0, len(seg), CHUNK):
        s, u = seg[start:start + CHUNK], t[start:start + CHUNK, None]
        xy = pa[s] + u * (pb[s] - pa[s])
        h[start:start + CHUNK] = terrain.heights(xy[:, 0], xy[:, 1])
    weight[~np.isfinite(h)] = 0
    h[~np.isfinite(h)] = 0
    support = np.bincount(a[seg], weight * (1 - t), n) + np.bincount(b[seg], weight * t, n)
    known = support > 0

    # Rows that pin a combination of variables to a value: the approaches of side roads, and the fords.
    pins = ([], [], [], [])  # row, variable, coefficient, value per row

    def pin(variables, coefficients, value):
        if all(known[v] for v in variables):
            row = len(pins[3])
            pins[0].extend([row] * len(variables))
            pins[1].extend(variables)
            pins[2].extend(coefficients)
            pins[3].append(value)

    for (w, j, step), (through, distance) in ways.approaches.items():
        vertices, index = ways.vertex[w], ways.index[w][j]
        origin = ways.point[w][index]
        position = index + step
        while 0 <= position < len(vertices):
            p = ways.point[w][position]
            if np.hypot(*(p - origin)) * metres(origin[1]) > distance + 1.01:
                break
            best = None
            for tw, tj, tstep in through:
                ti = ways.index[tw][tj]
                q = ways.point[tw][ti + tstep]
                axis = q - ways.point[tw][ti]
                tau = float((p - origin) @ axis / (axis @ axis))
                if best is None or tau > best[0]:
                    best = (min(max(tau, 0.0), 1.0), ways.vertex[tw][ti + tstep])
            tau, other = best
            pin([vertices[position], vertices[index], other], [1.0, -(1 - tau), -tau], 0.0)
            position += step
    approaches = len(pins[3])
    for variables, coefficients, value in ford_pins(ways, rivers):
        pin(variables, coefficients, value)

    # Smoothness: per vertex inside a way, the change of grade over the mean of its two segments.
    starts = np.cumsum([0] + [len(v) - 1 for v in ways.vertex])
    inner = np.concatenate([np.arange(s0, s1 - 1) for s0, s1 in zip(starts[:-1], starts[1:])]).astype(np.int64)
    inner = inner[known[a[inner]] & known[b[inner]] & known[b[inner + 1]]]
    la, lb = length[inner], length[inner + 1]
    lam = np.array([rule["smooth_m"] ** 4 for rule in rules])[seg_way[inner]] / ((la + lb) / 2)
    rows = np.repeat(np.arange(len(inner)), 3)
    cols = np.column_stack([a[inner], b[inner], b[inner + 1]]).ravel()
    vals = np.column_stack([1 / la, -1 / la - 1 / lb, 1 / lb]).ravel()
    D2 = sparse.csr_matrix((vals, (rows, cols)), shape=(len(inner), n))
    smooth = (D2.T @ sparse.diags(lam) @ D2)
    # Grades: per segment of a class with a limit, z_b − z_a within ± grade × length.
    grade = np.array([rule["grade"] if rule["grade"] is not None else np.inf for rule in rules])[seg_way]
    limited = np.flatnonzero(np.isfinite(grade) & known[a] & known[b] & (length > 0))
    G = sparse.csr_matrix((np.tile([-1.0, 1.0], len(limited)), (np.repeat(np.arange(len(limited)), 2), np.column_stack([a[limited], b[limited]]).ravel())), shape=(len(limited), n))
    bound = grade[limited] * length[limited]
    P = sparse.csr_matrix((pins[2], (pins[0], pins[1])), shape=(len(pins[3]), n))
    fixed = smooth + PIN * (P.T @ P) + sparse.diags(np.where(known, 0.0, 1.0))
    pinned = PIN * (P.T @ np.asarray(pins[3]))

    # Rounds of the smooth solve and the slope limits: a segment steeper than its class holds its
    # limit, and a held segment that the solve would make less steep lets go. Each round also takes
    # the Huber weights of the round before.
    held = np.zeros(len(limited))
    w = weight.copy()
    for rounds in range(1, ROUNDS + 1):
        # The samples of a segment, summed: they act on its two ends only.
        ca, cb, m = 1 - t, t, len(a)
        aa, bb, ab = (np.bincount(seg, w * c, m) for c in (ca * ca, cb * cb, ca * cb))
        data = sparse.csr_matrix((np.concatenate([aa, bb, ab, ab]), (np.concatenate([a, b, a, b]), np.concatenate([a, b, b, a]))), shape=(n, n))
        rhs = np.bincount(a, np.bincount(seg, w * ca * h, m), n) + np.bincount(b, np.bincount(seg, w * cb * h, m), n) + pinned
        on = np.flatnonzero(held)
        limits = G[on]
        matrix = data + fixed + PIN * (limits.T @ limits)
        z = splu(matrix.tocsc(), permc_spec="MMD_AT_PLUS_A", diag_pivot_thresh=0.0, options={"SymmetricMode": True}).solve(rhs + PIN * (limits.T @ (held[on] * bound[on])))
        residual = np.abs(z[a[seg]] * (1 - t) + z[b[seg]] * t - h)
        reweighted = weight * np.minimum(1.0, HUBER_M / np.maximum(residual, 1e-9))
        gz = G @ z
        before = held.copy()
        held[held * gz < bound] = 0
        held[gz > bound + TOLERANCE_M] = 1
        held[gz < -bound - TOLERANCE_M] = -1
        if rounds >= HUBER_ROUNDS and np.array_equal(held, before):
            break
        w = reweighted if rounds < HUBER_ROUNDS else w

    out = [[(outer, points, None) for outer, points, _ in parts] for _, _, parts in features]
    for wi, (f, p, _, _, _, _) in enumerate(ways.ways):
        keep = kept(ways, wi, z, known)
        heights = [float(z[v]) if known[v] else None for v in ways.vertex[wi][keep]]
        out[f][p] = (out[f][p][0], [tuple(point) for point in ways.point[wi][keep]], heights)
    culverts = set()
    first = np.cumsum(k) - k
    for wi, (f, _, _, _, free_way, _) in enumerate(ways.ways):
        kind, tags, _ = features[f]
        s0, s1 = starts[wi], starts[wi + 1]
        if free_way and kind != "tunnel" and length[s0:s1].sum() < CULVERT_M:
            samples = np.arange(first[s0], first[s1 - 1] + k[s1 - 1])
            samples = samples[weight[samples] > 0]
            deck = z[a[seg[samples]]] * (1 - t[samples]) + z[b[seg[samples]]] * t[samples]
            if samples.size and np.max(deck - h[samples]) <= CULVERT_RISE_M:
                culverts.add(f)
    excess = np.abs(G @ z) - bound
    metrics = {
        "ways": len(ways.ways), "variables": n, "known": int(known.sum()), "approaches": approaches,
        "fords": len(pins[3]) - approaches, "culverts": len(culverts), "rounds": rounds, "held": int(np.count_nonzero(held)),
        "grade_excess_m": round(float(np.max(excess, initial=0)), 4),
        "residual_p99_m": round(float(np.percentile(residual[weight > 0], 99)), 2) if (weight > 0).any() else 0,
    }
    return out, culverts, metrics


def kept(ways, w, z, known):
    """The positions of the vertices of a way that the tiles keep: those of OSM, and an added vertex where
    the profile leaves the straight line between its neighbours by more than KEEP_M (Douglas-Peucker in
    the plane of distance and height)."""
    osm, points = ways.osm_vertex[w], ways.point[w]
    keep = osm.copy()
    along = np.concatenate([[0], np.cumsum(np.hypot(*np.diff(points, axis=0).T) * metres(points[:-1, 1]))])
    heights = np.where(known[ways.vertex[w]], z[ways.vertex[w]], np.nan)
    index = ways.index[w]
    for i, j in zip(index[:-1], index[1:]):
        if j - i < 2:
            continue
        if not np.isfinite(heights[i:j + 1]).all():
            keep[i:j + 1] = True
            continue
        spans = [(i, j)]
        while spans:
            s, e = spans.pop()
            if e - s < 2:
                continue
            inner = np.arange(s + 1, e)
            line = heights[s] + (heights[e] - heights[s]) * (along[inner] - along[s]) / (along[e] - along[s])
            far = np.argmax(np.abs(heights[inner] - line))
            if abs(heights[inner][far] - line[far]) > KEEP_M:
                m = inner[far]
                keep[m] = True
                spans += [(s, m), (m, e)]
    return keep


def ford_pins(ways, rivers):
    """(variables, coefficients, value) per ford: a road vertex that a waterway shares, or the point where
    a way tagged as a ford crosses a waterway, lies FORD_M below the water there."""
    if not rivers:
        return []
    levels = {}
    segments = []
    for points, z in rivers:
        for point, level in zip(points, z):
            if level is not None and math.isfinite(level):
                levels.setdefault(tuple(point), level)
        for (p, zp), (q, zq) in zip(zip(points, z), zip(points[1:], z[1:])):
            if zp is not None and zq is not None:
                segments.append((p, q, zp, zq))
    # A grid of the waterway segments, about GRID_UNITS of world units a cell, for the crossings of fords.
    grid = {}
    for number, (p, q, _, _) in enumerate(segments):
        for cx in range(int(min(p[0], q[0]) // GRID_UNITS), int(max(p[0], q[0]) // GRID_UNITS) + 1):
            for cy in range(int(min(p[1], q[1]) // GRID_UNITS), int(max(p[1], q[1]) // GRID_UNITS) + 1):
                grid.setdefault((cx, cy), []).append(number)
    out = []
    for w, (_, _, points, _, free, ford) in enumerate(ways.ways):
        if free:
            continue
        vertices, index = ways.vertex[w], ways.index[w]
        for j, point in enumerate(points):
            level = levels.get(tuple(point))
            if level is not None:
                out.append(([vertices[index[j]]], [1.0], level - FORD_M))
        if not ford:
            continue
        for s in range(len(points) - 1):
            p, q = points[s], points[s + 1]
            near = {number for cx in range(int(min(p[0], q[0]) // GRID_UNITS), int(max(p[0], q[0]) // GRID_UNITS) + 1)
                    for cy in range(int(min(p[1], q[1]) // GRID_UNITS), int(max(p[1], q[1]) // GRID_UNITS) + 1) for number in grid.get((cx, cy), [])}
            for r0, r1, z0, z1 in (segments[number] for number in sorted(near)):
                hit = crossing(p, q, np.asarray(r0), np.asarray(r1))
                if hit is not None:
                    tp, tr = hit
                    # The crossing on the way's own segment lies between its vertices s and s + 1.
                    lo, hi = index[s], index[s + 1]
                    along = np.hypot(*(ways.point[w][lo:hi + 1] - p).T)
                    total = along[-1]
                    target = tp * total
                    m = min(max(int(np.searchsorted(along, target)) - 1, 0), hi - lo - 1)
                    u = (target - along[m]) / max(along[m + 1] - along[m], 1e-12)
                    out.append(([vertices[lo + m], vertices[lo + m + 1]], [1 - u, u], z0 + (z1 - z0) * tr - FORD_M))
    return out


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
