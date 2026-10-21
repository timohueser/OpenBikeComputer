"""The water levels of the TrailView feature tiles, from the master terrain: each lake at one level, and
the rivers and streams as one network whose levels never rise downstream. specs/trailview-features.md
says what the levels promise."""

import math

import numpy as np

# The waterways and the areas that the solver reads: river areas give widths; falls, weirs and dams
# keep the smoothing of a level from crossing them.
RIVERS = {"river", "stream"}
LAKES = {"lake"}
AREAS = {"river-area"}
MARKS = {"waterfall", "dam", "dam-point"}
# As WATERWAYS of the engine's bins.ts: the half-width of a waterway without a `width` tag.
HALF_WIDTHS = {"river": 5.0, "canal": 5.0, "stream": 1.0}
# A river observes the terrain every STEP_M along it, at its lowest within ±(w/2 + BESIDE_M) across it,
# at ACROSS points.
STEP_M = 5.0
BESIDE_M = 2.0
ACROSS = 5
# The fit holds z + LEAST_GRADE × s from rising downstream, so that a pool still falls a little and the
# smoothing does not stair-step it.
LEAST_GRADE = 0.0005
# The smoothing: a triangle kernel of this half-width.
SMOOTH_M = 25.0
# A way turns round when its terrain rises by more than this from end to end: OSM draws it uphill.
UPHILL_M = 5.0
# A sample in a lake, or this near its shore, holds the lake's level with this weight.
SHORE_M = 2.0
PIN = 1000.0
# A fall, a weir or a dam this near a sample splits the smoothing there.
FALL_M = 3.0
# Lakes: the median of the interior holds where this share of it lies this near it; else a shore
# quantile, a little inside. An `ele` tag this near the level wins.
FLAT_M = 0.3
FLAT_SHARE = 0.8
INSET_M = 2.0
SHORE_QUANTILE = 15
ELE_M = 1.5
# A lake spills where its shore, away from its rivers, lies this far below its level.
SPILL_M = 0.5
RIVER_NEAR_M = 30.0
# A lake reads the terrain at no more than this many points.
LAKE_POINTS = 250_000
TILE = 512
EQUATOR_M = 40075016.686


def reads(kind):
    """Whether the solver reads a feature of this kind: it sets the levels of rivers and lakes."""
    return kind in RIVERS | LAKES | AREAS | MARKS


def metres(y):
    """Metres per world unit at world row y: the Mercator scale."""
    return EQUATOR_M * np.cos(np.arctan(np.sinh(np.pi * (1 - 2 * y))))


def falling(y, w):
    """The weighted least-squares fit to y that never rises (pool-adjacent violators)."""
    means, weights, counts = [], [], []
    for value, weight in zip(y, w):
        means.append(value)
        weights.append(weight)
        counts.append(1)
        while len(means) > 1 and means[-2] < means[-1]:
            weight = weights[-2] + weights[-1]
            means[-2] = (means[-2] * weights[-2] + means[-1] * weights[-1]) / weight
            weights[-2] = weight
            counts[-2] += counts[-1]
            del means[-1], weights[-1], counts[-1]
    return np.repeat(means, counts)


def smoothed(z, splits):
    """z under a triangle kernel of SMOOTH_M, with each end held: a positive kernel keeps a profile that
    never rises. The pieces between splits are smoothed apart."""
    k = int(SMOOTH_M // STEP_M)
    kernel = 1 - np.abs(np.arange(-k, k + 1)) / (k + 1)
    kernel /= kernel.sum()
    bounds = [0, *splits, len(z)]
    return np.concatenate([np.convolve(np.pad(z[a:b], k, mode="edge"), kernel, "valid") for a, b in zip(bounds, bounds[1:]) if b > a])


class Way:
    """A river part: its points (world coordinates in [0, 1], y down) in the direction of its flow, the
    samples along it every STEP_M with the lowest terrain across it, and their weights."""

    def __init__(self, points, half, terrain):
        self.points, self.half = points, half
        xy = np.asarray(points, np.float64)
        d = np.diff(xy, axis=0)
        scale = metres((xy[:-1, 1] + xy[1:, 1]) / 2)
        lengths = np.hypot(*d.T) * scale
        steps = np.maximum(1, np.ceil(lengths / STEP_M)).astype(int)
        t = np.concatenate([np.arange(n) / n for n in steps] + [[1.0]])
        segment = np.concatenate([np.full(n, k) for k, n in enumerate(steps)] + [[len(d) - 1]])
        self.xy = xy[segment] + t[:, None] * d[segment]
        self.along = np.concatenate([[0], np.cumsum(lengths)])[segment] + t * lengths[segment]
        self.vertices = np.concatenate([[0], np.cumsum(steps)])
        self.bounds = (*self.xy.min(0), *self.xy.max(0))
        normal = np.stack([-d[:, 1], d[:, 0]], 1) / np.maximum(np.hypot(*d.T), 1e-15)[:, None]
        across = np.linspace(-1, 1, ACROSS) * (half + BESIDE_M)
        rows = [terrain.heights(*(self.xy + normal[segment] * (o / metres(self.xy[:, 1]))[:, None]).T) for o in across]
        with np.errstate(all="ignore"):
            self.low = np.nanmin(np.stack(rows), 0).astype(np.float64)
        self.weight = np.where(np.isfinite(self.low), 1.0, 0.0)

    def uphill(self):
        """Whether the terrain along the way rises by more than UPHILL_M: a straight fit to its lows."""
        known = np.isfinite(self.low)
        if known.sum() < 2 or self.along[-1] <= 0:
            return False
        slope = np.polyfit(self.along[known], self.low[known], 1)[0]
        return slope * self.along[-1] > UPHILL_M


def contains(rings, x, y):
    """Per point, whether it lies inside the rings, even-odd."""
    import shapely

    inside = np.zeros(np.shape(x), bool)
    for ring in rings:
        if len(ring) >= 3:
            polygon = shapely.Polygon(ring)
            shapely.prepare(polygon)
            inside ^= shapely.contains_xy(polygon, x, y)
    return inside


def distance(rings, x, y):
    """Per point, the distance to the nearest edge of the rings, in world units."""
    import shapely

    points = shapely.points(np.asarray(x, np.float64), np.asarray(y, np.float64))
    best = np.full(np.shape(x), np.inf)
    for ring in rings:
        if len(ring) >= 3:
            best = np.minimum(best, shapely.distance(shapely.LinearRing(ring), points))
    return best


def box(rings, margin=0.0):
    xy = np.concatenate([np.asarray(ring, np.float64) for ring in rings])
    return (*(xy.min(0) - margin), *(xy.max(0) + margin))


def overlapping(boxes, bounds):
    """The indices of the boxes (N × 4: x0, y0, x1, y1) that meet the box `bounds`."""
    if not len(boxes):
        return []
    b = np.asarray(boxes)
    return np.flatnonzero((b[:, 0] <= bounds[2]) & (b[:, 2] >= bounds[0]) & (b[:, 1] <= bounds[3]) & (b[:, 3] >= bounds[1]))


def near_box(xy, bounds):
    x0, y0, x1, y1 = bounds
    return (xy[:, 0] >= x0) & (xy[:, 0] <= x1) & (xy[:, 1] >= y0) & (xy[:, 1] <= y1)


def ring_points(rings, step_m, offset_m):
    """Points every step_m along the rings, moved offset_m inside the area (negative: outside)."""
    points, normals = [], []
    for ring in rings:
        r = np.asarray(ring, np.float64)
        for a, b in zip(r, np.roll(r, -1, 0)):
            n = max(1, math.ceil(math.hypot(*(b - a)) * metres((a[1] + b[1]) / 2) / step_m))
            points.append(a + ((np.arange(n) + 0.5) / n)[:, None] * (b - a))
            normals.append(np.tile(np.array([a[1] - b[1], b[0] - a[0]]) / max(math.hypot(*(b - a)), 1e-15), (n, 1)))
    if not points:
        return np.zeros((0, 2))
    p, normal = np.concatenate(points), np.concatenate(normals)
    step = (abs(offset_m) / metres(p[:, 1]))[:, None]
    # The normal points inside where a point a hundredth of the step along it is inside.
    inward = contains(rings, *(p + normal * step / 100).T)
    sign = np.where(inward == (offset_m >= 0), 1.0, -1.0)[:, None]
    return p + sign * normal * step


def lake_level(tags, rings, terrain):
    """The level of a lake: the median of its interior where it is flat; else the SHORE_QUANTILE of its
    shore, INSET_M inside. A pond takes its lowest shore, a reservoir its shore at the outline, the full
    level. An `ele` tag within ELE_M wins. NaN where the terrain knows none of it."""
    with np.errstate(all="ignore"):
        shore = terrain.heights(*ring_points(rings, STEP_M, INSET_M).T)
        if tags.get("water") == "pond":
            level = np.nanmin(shore) if np.isfinite(shore).any() else math.nan
        elif tags.get("water") == "reservoir" or tags.get("landuse") == "reservoir":
            outline = terrain.heights(*ring_points(rings, STEP_M, 0.0).T)
            level = np.nanmedian(outline) if np.isfinite(outline).any() else math.nan
        else:
            h = interior(rings, terrain)
            h = h[np.isfinite(h)]
            median = np.median(h) if h.size else math.nan
            if h.size and np.mean(np.abs(h - median) <= FLAT_M) >= FLAT_SHARE:
                level = median
            elif np.isfinite(shore).any():
                level = np.nanpercentile(shore, SHORE_QUANTILE)
            else:
                level = median
    ele = number(tags.get("ele"))
    return float(ele if abs(ele - level) <= ELE_M else level)


def interior(rings, terrain):
    """The terrain on a grid inside the rings, at the stored resolution or LAKE_POINTS points at most."""
    x0, y0, x1, y1 = box(rings)
    area = sum(abs(np.sum(r[:, 0] * np.roll(r[:, 1], -1) - np.roll(r[:, 0], -1) * r[:, 1])) / 2
               for r in (np.asarray(ring, np.float64) for ring in rings))
    step = max(1 / ((1 << terrain.top) * TILE), math.sqrt(area / LAKE_POINTS))
    xs, ys = np.arange(x0 + step / 2, x1, step), np.arange(y0 + step / 2, y1, step)
    gx, gy = np.meshgrid(xs, ys)
    at = contains(rings, gx, gy) if xs.size and ys.size else np.zeros((0, 0), bool)
    return terrain.heights(gx[at], gy[at]) if at.any() else np.zeros(0)


def spills(level, rings, terrain, rivers):
    """Whether a lake's shore lies below its level by more than SPILL_M, INSET_M outside it and away from
    its rivers."""
    outside = ring_points(rings, STEP_M, -INSET_M)
    if not outside.size:
        return False
    h = terrain.heights(*outside.T)
    away = np.ones(len(outside), bool)
    for way in rivers:
        for x, y in way.points[:: max(1, len(way.points) // 64)] + [way.points[-1]]:
            away &= np.hypot(outside[:, 0] - x, outside[:, 1] - y) * metres(y) > RIVER_NEAR_M
    return bool(np.any(away & (h < level - SPILL_M)))


def solve(features, terrain):
    """Per feature (kind, tags, parts), the heights of each part: one level for a lake, one per vertex
    for a river, None for a part of a feature that the solver only reads. A river way that OSM draws uphill
    turns round in place, and a river without a `width` tag inside a river area takes the area's width.
    Returns the heights and the metrics."""
    lakes = []
    for index, (kind, tags, parts) in enumerate(features):
        if kind in LAKES:
            rings = [points for _, points, *_ in parts]
            lakes.append((index, rings, box(rings), lake_level(tags, rings, terrain)))
    areas = [[points for _, points, *_ in parts] for kind, _, parts in features if kind in AREAS]
    area_boxes = [box(rings) for rings in areas]
    marks = np.array([point for kind, _, parts in features if kind in MARKS for _, points, *_ in parts for point in points]
                     or np.zeros((0, 2)), np.float64)
    ways, turned = [], 0
    for index, (kind, tags, parts) in enumerate(features):
        if kind not in RIVERS:
            continue
        width = number(tags.get("width"))
        half = width / 2 if math.isfinite(width) else HALF_WIDTHS.get(tags.get("waterway"), 1.0)
        mine = []
        for n, (_, points, *_) in enumerate(parts):
            if len(points) < 2:
                continue
            way = Way(points, half, terrain)
            if way.uphill():
                points.reverse()
                way = Way(points, half, terrain)
                turned += 1
            way.feature, way.part = index, n
            mine.append(way)
        ways += mine
        if not math.isfinite(width) and kind == "river":
            half = river_area_half(mine, areas, area_boxes)
            if half:
                tags["width"] = f"{2 * half:.1f}"
    boxes = [way.bounds for way in ways]
    pin(ways, boxes, lakes)
    levels = network(ways, marks)
    out = [[None] * len(parts) for _, _, parts in features]
    for index, _, _, level in lakes:
        out[index] = [[level if math.isfinite(level) else None] * len(points) for _, points, *_ in features[index][2]]
    for way in ways:
        z = levels[id(way)][way.vertices]
        out[way.feature][way.part] = [float(v) if math.isfinite(v) else None for v in z]
    spilling = 0
    for _, rings, bounds, level in lakes:
        margin = 2 * RIVER_NEAR_M / metres(bounds[1])
        near = [ways[i] for i in overlapping(boxes, (bounds[0] - margin, bounds[1] - margin, bounds[2] + margin, bounds[3] + margin))]
        spilling += math.isfinite(level) and spills(level, rings, terrain, near)
    return out, {"lakes": len(lakes), "lakes_spilling": int(spilling), "river_parts": len(ways), "turned": turned}


def number(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return math.nan


def river_area_half(ways, areas, area_boxes):
    """The half-width of a river inside river areas: the median distance from its samples there to the
    areas' edges, None where less than a third of it lies inside one."""
    distances, total = [], 0
    for way in ways:
        total += len(way.xy)
        for i in overlapping(area_boxes, way.bounds):
            rings = areas[i]
            near = near_box(way.xy, area_boxes[i])
            if not near.any():
                continue
            x, y = way.xy[near].T
            at = contains(rings, x, y)
            distances.extend(distance(rings, x[at], y[at]) * metres(y[at]))
    return float(np.median(distances)) if total and len(distances) * 3 >= total else None


def pin(ways, boxes, lakes):
    """A sample in a lake, or within SHORE_M of its shore, holds the lake's level with the weight PIN."""
    for _, rings, bounds, level in lakes:
        if not math.isfinite(level):
            continue
        for i in overlapping(boxes, bounds):
            way = ways[i]
            near = near_box(way.xy, bounds)
            if not near.any():
                continue
            x, y = way.xy[near].T
            at = contains(rings, x, y) | (distance(rings, x, y) * metres(y) < SHORE_M)
            index = np.flatnonzero(near)[at]
            way.low[index], way.weight[index] = level, PIN


def network(ways, marks):
    """The levels of every sample of the ways. The ways split into reaches at the vertices that they
    share; the reaches are solved downstream in turn, each held under the levels that flow into its
    start, and all reaches at a junction then take one level there."""
    counts = {}
    for way in ways:
        for point in way.points:
            counts[point] = counts.get(point, 0) + 1
    reaches, junctions = [], {}
    for way in ways:
        cuts = [0, *(i for i in range(1, len(way.points) - 1) if counts[way.points[i]] > 1), len(way.points) - 1]
        for a, b in zip(cuts, cuts[1:]):
            reaches.append((way, way.vertices[a], way.vertices[b], way.points[a], way.points[b]))
            junctions.setdefault(way.points[a], ([], []))[1].append(len(reaches) - 1)
            junctions.setdefault(way.points[b], ([], []))[0].append(len(reaches) - 1)
    levels = {id(way): np.full(len(way.xy), np.nan) for way in ways}
    waiting = [len(junctions[start][0]) for *_, start, _ in reaches]
    ready, done, ends = [r for r, count in enumerate(waiting) if count == 0], [False] * len(reaches), {}
    while not all(done):
        # A loop of channels has no reach without one upstream of it: one of them goes as it comes.
        r = ready.pop() if ready else done.index(False)
        if done[r]:
            continue
        done[r] = True
        way, a, b, start, end = reaches[r]
        z = reach_levels(way, a, b, min(ends.get(start, [math.inf])), marks)
        levels[id(way)][a:b + 1] = z
        ends.setdefault(end, []).append(z[-1])
        for after in junctions[end][1]:
            waiting[after] -= 1
            if waiting[after] == 0:
                ready.append(after)
    # One level at a junction: the highest start that flows out of it, which lies below every level
    # that flows into it; where nothing flows out, the lowest level that flows in.
    for into, out in junctions.values():
        starts = [levels[id(reaches[r][0])][reaches[r][1]] for r in out]
        known = [v for v in starts if math.isfinite(v)]
        level = max(known) if known else min((levels[id(reaches[r][0])][reaches[r][2]] for r in into), default=math.nan)
        if not math.isfinite(level):
            continue
        for r in into:
            levels[id(reaches[r][0])][reaches[r][2]] = min(levels[id(reaches[r][0])][reaches[r][2]], level)
        for r in out:
            levels[id(reaches[r][0])][reaches[r][1]] = level
    return levels


def reach_levels(way, a, b, cap, marks):
    """The levels of the samples a..b of a way: the fit that never rises to the lows plus LEAST_GRADE
    per metre, under the cap, smoothed apart at falls, weirs and dams."""
    low, weight, along = way.low[a:b + 1], way.weight[a:b + 1], way.along[a:b + 1]
    known = weight > 0
    if not known.any():
        return np.full(b - a + 1, np.nan)
    s = along - along[0]
    fit = falling(low[known] + LEAST_GRADE * s[known], weight[known]) - LEAST_GRADE * s[known]
    z = np.interp(s, s[known], fit)
    z = np.minimum.accumulate(np.minimum(z, cap))
    splits = []
    if len(marks):
        xy = way.xy[a:b + 1]
        near = np.zeros(len(xy), bool)
        for x, y in marks[near_box(marks, (*xy.min(0) - 1e-5, *xy.max(0) + 1e-5))]:
            near |= np.hypot(xy[:, 0] - x, xy[:, 1] - y) * metres(y) < FALL_M
        splits = [int(i) for i in np.flatnonzero(near) if 0 < i < len(xy)]
    return smoothed(z, splits)
