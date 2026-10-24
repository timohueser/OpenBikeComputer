"""The aerialways of the TrailView feature tiles: their stations and pylons. OSM maps the stations of a lift
at the ends of its way and a pylon at each vertex between, but often maps no pylon. The bake adds a pylon
in a span that is longer than its rig allows, or under whose cable a carrier comes nearer the terrain than
its clearance. The pylon stands where the carrier comes nearest, as a real pylon stands on a knoll. A way
that runs through the end of another lift splits there: that vertex is a station, not a pylon."""

import math

import numpy as np

from .profiles import metres

# Per rig: the height of the cable over the ground at a pylon and at a station, the longest span, the sag
# at mid-span as a share of the span, and how far a carrier hangs under the cable and the clearance that
# it keeps over the terrain. As RIGS of builder/web/src/lib/trailview/kinds/aerialway.ts and its
# CABLE_AT_STATION_M of models/aerialways.ts.
RIGS = {
    "cable_car": {"pylon": 30.0, "station": 8.4, "span": math.inf, "sag": 0.04, "drop": 5.3, "clear": 10.0},
    "gondola": {"pylon": 14.0, "station": 6.0, "span": 300.0, "sag": 0.02, "drop": 3.7, "clear": 4.0},
    "chair_lift": {"pylon": 10.0, "station": 4.2, "span": 180.0, "sag": 0.02, "drop": 4.0, "clear": 2.5},
    "drag_lift": {"pylon": 7.0, "station": 4.5, "span": 120.0, "sag": 0.01, "drop": 2.9, "clear": 1.0},
}
# The rig of each `aerialway` value.
CLASSES = {"cable_car": "cable_car", "gondola": "gondola", "mixed_lift": "gondola", "chair_lift": "chair_lift",
           "drag_lift": "drag_lift", "t-bar": "drag_lift", "j-bar": "drag_lift", "platter": "drag_lift"}
# Samples of the terrain per span; an added pylon stands at least GAP_M from the pylons beside it, and a
# lift has at most MOST pylons.
SAMPLES, GAP_M, MOST = 16, 25.0, 200


def solves(kind, tags):
    return kind == "aerialway" and tags.get("aerialway") in CLASSES


def pylons(tags, points, terrain):
    """The vertices of a lift's way with the added pylons, in world coordinates."""
    rig = RIGS[CLASSES[tags["aerialway"]]]
    out = [tuple(p) for p in points]

    def ground(ps):
        return terrain.heights(np.array([p[0] for p in ps]), np.array([p[1] for p in ps]))

    z = list(ground(out) + [rig["station"] if i in (0, len(out) - 1) else rig["pylon"] for i in range(len(out))])
    t = np.arange(1, SAMPLES) / SAMPLES
    i = 0
    while i + 1 < len(out) and len(out) < MOST:
        (ax, ay), (bx, by) = out[i], out[i + 1]
        span = math.hypot(bx - ax, by - ay) * float(metres((ay + by) / 2))
        cable = z[i] + (z[i + 1] - z[i]) * t - 4 * rig["sag"] * span * t * (1 - t)
        below = ground([(ax + (bx - ax) * u, ay + (by - ay) * u) for u in t]) + rig["drop"] + rig["clear"] - cable
        # A pylon may stand only GAP_M or more from both ends of the span.
        room = np.isfinite(below) & (span * np.minimum(t, 1 - t) >= GAP_M)
        worst = int(np.argmax(np.where(room, below, -np.inf))) if room.any() else 0
        at = 0.5 if span > rig["span"] else float(t[worst])
        if (span > rig["span"] or (room.any() and below[worst] > 0)) and span * min(at, 1 - at) >= GAP_M:
            p = (ax + (bx - ax) * at, ay + (by - ay) * at)
            out.insert(i + 1, p)
            z.insert(i + 1, float(ground([p])[0]) + rig["pylon"])
        else:
            i += 1
    return out


def rigged(ways, buildings, terrain=None):
    """The lifts (tags, points) as the tiles keep them, in world coordinates: each way split at its vertices
    where another lift ends, with the pylons that it lacks when there is a `terrain`, and the tag `stations`
    where an end has no station of its own: the end lies in one of the `buildings` (outer rings), or a lift
    before it in the list has its station there. Also the metrics."""
    ends = {p for _, points in ways for p in (points[0], points[-1])}
    split = []
    for tags, points in ways:
        cut = [0] + [i for i in range(1, len(points) - 1) if points[i] in ends] + [len(points) - 1]
        split += [(tags, points[a:b + 1]) for a, b in zip(cut, cut[1:])]
    housed = inside(ends, buildings)
    held, out, metrics = set(), [], {"lifts": len(split), "added_pylons": 0, "housed_ends": 0, "shared_ends": 0}
    for tags, points in split:
        own = []
        for p in (points[0], points[-1]):
            own.append(p not in housed and p not in held)
            metrics["housed_ends"] += p in housed
            metrics["shared_ends"] += p not in housed and p in held
            held.add(p)
        if terrain is not None:
            rig = pylons(tags, points, terrain)
            metrics["added_pylons"] += len(rig) - len(points)
            points = rig
        stations = {(True, False): "start", (False, True): "end", (False, False): "none"}.get(tuple(own))
        out.append(({**tags, "stations": stations} if stations else tags, points))
    return out, metrics


def inside(points, rings):
    """The points that lie inside one of the rings."""
    points = list(points)
    if not points or not rings:
        return set()
    xy = np.array(points)
    boxes = np.array([(r[:, 0].min(), r[:, 1].min(), r[:, 0].max(), r[:, 1].max()) for r in rings])
    out = set()
    for p, (x, y) in zip(points, xy):
        for k in np.flatnonzero((boxes[:, 0] <= x) & (x <= boxes[:, 2]) & (boxes[:, 1] <= y) & (y <= boxes[:, 3])):
            ring = rings[k]
            ax, ay, bx, by = ring[:, 0], ring[:, 1], np.roll(ring[:, 0], -1), np.roll(ring[:, 1], -1)
            crosses = (ay > y) != (by > y)
            if np.count_nonzero(crosses & (x < ax + (y - ay) * (bx - ax) / np.where(by == ay, 1, by - ay))) % 2:
                out.add(p)
                break
    return out
