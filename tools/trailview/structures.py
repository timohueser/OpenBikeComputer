"""The bridges and galleries of the TrailView feature tiles: piers, deck depths and the hill side of a
gallery. A bridge is a chain of bridge ways that
meet end to end. Its piers stand at most SPAN_M apart and never in water that the bridge crosses for less
than CHANNEL_M; its deck is as deep as its longest span asks. A long footbridge high over the ground hangs
from cables and has no piers. The engine draws the deck along the ways and a pier at each `bridge-pier`
point, from the deck down into the ground."""

import math

import numpy as np

from .profiles import metres
from .water import contains

# OSM bridge values of a way that lies on the ground.
GROUND = {"no", "boardwalk", "low_water_crossing"}
KINDS = {"main-road", "road", "path"}
# The longest span between piers, and the deck depth per metre of span within its least and greatest.
SPAN_M = {"road": 45.0, "path": 30.0}
DEPTH = {"road": (1 / 30, 0.5, 2.5), "path": (1 / 40, 0.3, 1.0)}
# Water crossed for less than CHANNEL_M takes no pier; a pier stands PIER_GAP_M beside it. A pier
# keeps MIN_SPAN_M from another pier and from the bridge's ends.
CHANNEL_M, PIER_GAP_M, MIN_SPAN_M = 30.0, 2.0, 6.0
# A pier keeps this far from a waterway's edge.
BANK_M = 1.0
# A footbridge longer than CABLE_M that runs more than HIGH_M over the ground hangs from cables, as one that
# OSM tags so.
CABLE_M, HIGH_M = 40.0, 25.0
CABLES = {"suspension", "simple-suspension", "cable-stayed"}
STEP_M = 1.0


def is_bridge(kind, tags):
    return kind in KINDS and tags.get("bridge", "no") not in GROUND


def chains(ways):
    """The bridges: lists of (way, reversed) whose parts meet end to end, where exactly two bridge ends
    meet. `ways` lists (kind, tags, points) of the bridge ways."""
    ends = {}
    for w, (_, _, points) in enumerate(ways):
        for end in (0, 1):
            ends.setdefault(points[-end], []).append((w, end))
    seen, out = set(), []

    def neighbour(w, end):
        meeting = ends[ways[w][2][-end]]
        return next(((o, e) for o, e in meeting if o != w), None) if len(meeting) == 2 else None

    for start in range(len(ways)):
        if start in seen:
            continue
        # Walk back to the first way of the chain, then forward along it.
        w, end, steps = start, 0, 0
        while (step := neighbour(w, end)) and step[0] != start and steps < len(ways):
            w, end = step[0], 1 - step[1]
            steps += 1
        chain, flipped = [], end == 1
        while w not in seen:
            seen.add(w)
            chain.append((w, flipped))
            step = neighbour(w, 0 if flipped else 1)
            if not step:
                break
            w, flipped = step[0], step[1] == 1
        out.append(chain)
    return out


def wet_runs(xy, waterways, areas):
    """The runs of the samples `xy` (world units) that lie in water: [(first, last)] sample indices.
    `waterways` and `areas` hold (box, half-width, line) and (box, rings)."""
    wet = np.zeros(len(xy), bool)
    scale = metres(xy[:, 1])
    reach = 50 / scale.min()
    near = lambda box: box[0] <= xy[:, 0].max() + reach and box[2] >= xy[:, 0].min() - reach and box[1] <= xy[:, 1].max() + reach and box[3] >= xy[:, 1].min() - reach
    for box, half, line in waterways:
        if not near(box):
            continue
        a, b = line[:-1], line[1:]
        d = b - a
        for i in range(0, len(xy), 4096):
            p = xy[i:i + 4096, None, :]
            t = np.clip(np.sum((p - a) * d, 2) / np.maximum(np.sum(d * d, 1), 1e-30), 0, 1)
            gap = np.min(np.hypot(*np.moveaxis(p - a - t[..., None] * d, 2, 0)), 1) * scale[i:i + 4096]
            wet[i:i + 4096] |= gap <= half + BANK_M
    for box, rings in areas:
        if near(box):
            wet |= contains(rings, xy[:, 0], xy[:, 1])
    edges = np.flatnonzero(np.diff(np.concatenate([[0], wet.astype(np.int8), [0]])))
    return list(zip(edges[::2], edges[1::2] - 1))


def bridges(ways, waterways, areas, terrain):
    """Per bridge way, the tags that the bake adds: its deck depth in metres, and `bridge:structure` for a
    cable bridge; and the piers as (point, deck height there, a way of their bridge). `ways` lists (kind,
    tags, points, z) with the solved heights, `waterways` (half-width, points) and `areas` the rings of
    river areas and lakes, all in world coordinates."""
    added, piers, moved, hanging = [None] * len(ways), [], 0, 0
    bounds = lambda xy: (*xy.min(0), *xy.max(0))
    waterways = [(bounds(line), half, line) for half, points in waterways if len(line := np.asarray(points, np.float64)) >= 2]
    areas = [(bounds(np.concatenate([np.asarray(ring, np.float64) for ring in rings])), rings) for rings in areas]
    for chain in chains([(kind, tags, points) for kind, tags, points, _ in ways]):
        xy, z = [], []
        for w, flipped in chain:
            points, heights = ways[w][2], ways[w][3]
            order = slice(None, None, -1) if flipped else slice(None)
            xy += list(points[order])[1 if xy else 0:]
            z += list(heights[order])[1 if z else 0:]
        xy = np.asarray(xy, np.float64)
        lengths = np.hypot(*np.diff(xy, axis=0).T) * metres((xy[:-1, 1] + xy[1:, 1]) / 2)
        along = np.concatenate([[0], np.cumsum(lengths)])
        total = along[-1]
        kind = "path" if all(ways[w][0] == "path" for w, _ in chain) else "road"
        known = [i for i, h in enumerate(z) if h is not None]
        samples = np.arange(0, total + STEP_M / 2, STEP_M)
        at = np.stack([np.interp(samples, along, xy[:, 0]), np.interp(samples, along, xy[:, 1])], 1)
        structure = next((ways[w][1]["bridge:structure"] for w, _ in chain if ways[w][1].get("bridge:structure") in CABLES), None)
        if not structure and kind == "path" and total > CABLE_M and len(known) >= 2:
            deck = np.interp(samples, along[known], np.asarray(z, np.float64)[known])
            with np.errstate(invalid="ignore"):
                structure = "simple-suspension" if np.nanmax(deck - terrain.heights(at[:, 0], at[:, 1]), initial=0) > HIGH_M else None
        hanging += structure is not None
        spans = 1 if structure else max(1, math.ceil(total / SPAN_M[kind]))
        stations = [k * total / spans for k in range(1, spans)]
        if stations:
            for first, last in wet_runs(at, waterways, areas):
                lo, hi = samples[first], samples[last]
                if hi - lo >= CHANNEL_M:
                    continue
                for k, s in enumerate(stations):
                    if lo - PIER_GAP_M < s < hi + PIER_GAP_M:
                        stations[k] = lo - PIER_GAP_M if s - lo < hi - s else hi + PIER_GAP_M
                        moved += 1
            kept = []
            for s in sorted(stations):
                if MIN_SPAN_M <= s <= total - MIN_SPAN_M and (not kept or s - kept[-1] >= MIN_SPAN_M):
                    kept.append(s)
            stations = kept
        stops = [0.0, *stations, total]
        rate, least, most = DEPTH[kind]
        depth = least if structure else round(min(most, max(least, max(b - a for a, b in zip(stops, stops[1:])) * rate)), 1)
        for w, _ in chain:
            added[w] = {"deck": f"{depth:.1f}", **({"bridge:structure": structure} if structure else {})}
        if len(known) < 2:
            continue
        for s in stations:
            point = (float(np.interp(s, along, xy[:, 0])), float(np.interp(s, along, xy[:, 1])))
            piers.append((point, float(np.interp(s, along[known], np.asarray(z, np.float64)[known])), chain[0][0]))
    return added, piers, {"bridges": sum(a is not None for a in added), "cable_bridges": hanging, "piers": len(piers), "piers_moved": moved}


# A gallery compares the terrain this far to each side of its way, every GALLERY_STEP_M.
BESIDE_M, GALLERY_STEP_M = 10.0, 5.0


def uphill(points, terrain):
    """The side of a gallery that the hill rises on, "left" or "right" of its way, from the terrain
    BESIDE_M to each side of it."""
    xy = np.asarray(points, np.float64)
    d = np.diff(xy, axis=0)
    lengths = np.hypot(*d.T)
    steps = np.maximum(1, np.ceil(lengths * metres(xy[:-1, 1]) / GALLERY_STEP_M)).astype(int)
    segment = np.repeat(np.arange(len(d)), steps)
    t = np.concatenate([np.arange(n) / n for n in steps])
    at = xy[segment] + t[:, None] * d[segment]
    left = np.stack([d[segment, 1], -d[segment, 0]], 1) / np.maximum(lengths[segment], 1e-15)[:, None]
    reach = (BESIDE_M / metres(at[:, 1]))[:, None]
    with np.errstate(invalid="ignore"):
        rise = np.nanmean(terrain.heights(*(at + left * reach).T) - terrain.heights(*(at - left * reach).T))
    return "right" if rise < 0 else "left"
