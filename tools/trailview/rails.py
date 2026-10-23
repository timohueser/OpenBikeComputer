"""The railway furniture of the TrailView feature tiles: the masts of the contact wire of the electrified
tracks, and the heights of the platforms. Masts stand evenly along each track, at most MAST_M apart, on the
side away from a neighbouring track; a track with neighbours on both sides takes the masts of the outer
tracks. A platform's top lies PLATFORM_M over the rail of the track nearest to each of its vertices."""

import numpy as np
from scipy.spatial import cKDTree

from .profiles import metres

KINDS = {"platform", "platform-line"}
# The most metres between two masts of a track, and how far beside the track's middle a mast stands, for
# a standard and for a narrow gauge.
MAST_M = 50.0
MAST_SIDE_M = {False: 3.0, True: 2.5}
# A track within NEIGHBOUR_M beside a mast, but farther than NEAR_M from its track, is a neighbour.
NEIGHBOUR_M, NEAR_M = 8.0, 1.0
SAMPLE_M = 2.0
# A platform's top over the rail, for a standard and for a narrow gauge; a vertex reads the nearest track
# within PLATFORM_REACH_M.
PLATFORM_M = {False: 0.55, True: 0.35}
PLATFORM_REACH_M = 15.0
NARROW_MM = 1200


def solves(kind, tags):
    """Whether this module sets the heights of a feature."""
    return kind in KINDS


def narrow(tags):
    """Whether a track is narrower than standard gauge: by its `gauge` in mm, else by its class."""
    gauge = str(tags.get("gauge", "")).split(";")[0]
    return int(gauge) < NARROW_MM if gauge.isdigit() else tags.get("railway") in {"narrow_gauge", "funicular"}


def electrified(tags):
    """Whether a track runs under a contact wire."""
    return tags.get("electrified") in {"contact_line", "yes"} and tags.get("railway") != "funicular"


def samples(tracks):
    """Points every SAMPLE_M along the tracks with heights, as world xy, heights, their track and the
    track's `narrow`. `tracks` lists (tags, points, z)."""
    xy, z, owner = [], [], []
    for t, (_, points, heights) in enumerate(tracks):
        p = np.asarray(points, np.float64)
        h = np.array([np.nan if v is None else v for v in heights])
        for a, b, ha, hb in zip(p[:-1], p[1:], h[:-1], h[1:]):
            n = max(1, int(np.ceil(np.hypot(*(b - a)) * metres(a[1]) / SAMPLE_M)))
            u = np.arange(n) / n
            xy.append(a + u[:, None] * (b - a))
            z.append(ha + u * (hb - ha))
            owner.append(np.full(n, t))
        xy.append(p[-1:])
        z.append(h[-1:])
        owner.append([t])
    if not xy:
        return np.zeros((0, 2)), np.zeros(0), np.zeros(0, np.int64)
    return np.concatenate(xy), np.concatenate(z), np.concatenate(owner).astype(np.int64)


def masts(tracks, beside):
    """The masts: (point, the height of its track's rail, tags) per mast. `tracks` lists (tags, points, z,
    on a deck) of the electrified tracks on the ground and on bridges; `beside` lists (tags, points, z) of
    every track out of a tunnel, the neighbours."""
    xy, _, _ = samples(beside)
    tree = cKDTree(xy) if len(xy) else None
    out = []
    for tags, points, z, deck in tracks:
        p = np.asarray(points, np.float64)
        h = np.array([np.nan if v is None else v for v in z])
        lengths = np.hypot(*np.diff(p, axis=0).T) * metres((p[:-1, 1] + p[1:, 1]) / 2)
        along = np.concatenate([[0], np.cumsum(lengths)])
        if along[-1] <= 0:
            continue
        # A centimetre over the spacing takes no extra mast.
        n = max(1, int(np.ceil((along[-1] - 0.01) / MAST_M)))
        side_m = MAST_SIDE_M[narrow(tags)]
        for s in (np.arange(n) + 0.5) * along[-1] / n:
            k = min(int(np.searchsorted(along, s, "right")) - 1, len(lengths) - 1)
            u = (s - along[k]) / max(lengths[k], 1e-9)
            at, height = p[k] + u * (p[k + 1] - p[k]), h[k] + u * (h[k + 1] - h[k])
            if not np.isfinite(height):
                continue
            d = p[k + 1] - p[k]
            left = np.array([d[1], -d[0]]) / max(np.hypot(*d), 1e-300)
            unit = metres(at[1])
            sides = set()
            for i in tree.query_ball_point(at, NEIGHBOUR_M / unit) if tree is not None else []:
                offset = float((xy[i] - at) @ left) * unit
                if abs(offset) > NEAR_M and np.hypot(*(xy[i] - at)) * unit > NEAR_M:
                    sides.add(1 if offset > 0 else -1)
            if len(sides) == 2:
                continue
            sign = -1 if sides == {1} else 1
            point = at + sign * left * side_m / unit
            out.append(((float(point[0]), float(point[1])), float(height), {"on": "deck"} if deck else {}))
    return out


def platforms(parts, tracks):
    """Per platform, its parts with a height on each vertex: PLATFORM_M over the rail of the nearest track
    within PLATFORM_REACH_M, or the middle height of its other vertices; None where no vertex has a track
    near. `parts` lists the (outer, points, None) of each platform, `tracks` (tags, points, z)."""
    xy, z, owner = samples(tracks)
    known = np.isfinite(z)
    xy, z, owner = xy[known], z[known], owner[known]
    tree = cKDTree(xy) if len(xy) else None
    lift = np.array([PLATFORM_M[narrow(tags)] for tags, *_ in tracks])
    out = []
    for platform in parts:
        rows = []
        for outer, points, _ in platform:
            p = np.asarray(points, np.float64)
            heights = np.full(len(p), np.nan)
            if tree is not None:
                d, i = tree.query(p, distance_upper_bound=PLATFORM_REACH_M / metres(p[:, 1].min()))
                hit = np.isfinite(d) & (d * metres(p[:, 1]) <= PLATFORM_REACH_M)
                heights[hit] = z[i[hit]] + lift[owner[i[hit]]]
            rows.append((outer, points, heights))
        found = np.concatenate([h for *_, h in rows])
        if not np.isfinite(found).any():
            out.append(None)
            continue
        middle = float(np.nanmedian(found))
        out.append([(outer, points, np.where(np.isfinite(h), h, middle).tolist()) for outer, points, h in rows])
    return out
