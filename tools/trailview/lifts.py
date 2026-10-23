"""The aerialways of the TrailView feature tiles: the pylons of each lift. OSM maps the stations of a lift
at the ends of its way and a pylon at each vertex between, but often maps no pylon. The bake adds a pylon
in a span that is longer than its rig allows, or under whose cable a carrier comes nearer the terrain than
its clearance. The pylon stands where the carrier comes nearest, as a real pylon stands on a knoll."""

import math

import numpy as np

from .profiles import metres

# Per rig: the height of the cable over the ground at a pylon and at a station, the longest span, the sag
# at mid-span as a share of the span, and how far a carrier hangs under the cable and the clearance that
# it keeps over the terrain. As RIGS of builder/web/src/lib/trailview/kinds/aerialway.ts.
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
        known = np.isfinite(below)
        worst = int(np.argmax(np.where(known, below, -np.inf))) if known.any() else 0
        at = 0.5 if span > rig["span"] else float(t[worst])
        if (span > rig["span"] or (known.any() and below[worst] > 0)) and span * min(at, 1 - at) > GAP_M:
            p = (ax + (bx - ax) * at, ay + (by - ay) * at)
            out.insert(i + 1, p)
            z.insert(i + 1, float(ground([p])[0]) + rig["pylon"])
        else:
            i += 1
    return out
