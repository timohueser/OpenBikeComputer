// A drawn ring into cells: which cells of a band intersect the closed polygon a
// lasso gesture traced (#1038's ghosted fourth tool, now real).
//
// **What this is, and what it deliberately is not.** It is a polygon-vs-square
// overlap test evaluated per candidate cell, all in integer microdegrees. It is
// not a metric buffer and needs no projection: unlike the corridor's
// distance-in-metres question, "does my square overlap this ring" is an affine
// question the lat/lon plane answers exactly, so there is no cosine anywhere in
// this file and nothing for latitude to distort.
//
// The overlap test has exactly two cases, and together they are complete for a
// simple ring against a convex square:
//
//   * **An edge of the ring overlaps the square** — decided by Liang–Barsky
//     clipping (the segment clipped to the square is non-empty). This covers
//     every partial overlap, and also a ring drawn entirely *inside* one cell,
//     whose edges lie in the square without crossing its boundary.
//   * **The square lies entirely inside the ring** — no edge touches it, so one
//     corner standing in for all four is tested even-odd against the ring, the
//     same rule the map's fill and the store's `pointInRings` use.
//
// Edges are treated as closed against the half-open cell squares; the µdeg of
// slack that costs errs toward including a cell, the same attitude the corridor
// test takes at its shared edges.

import { cellsIntersecting, cellSquare, GridError, type CellId, type UBox } from "./grid";
import type { LatLon } from "./corridor";

/** The widest a lasso may reach in longitude, µdeg: half the world — the same
 *  antimeridian refusal the corridor makes, for the same reason (OBCA §1.4: the
 *  grid does not wrap, so a ring across the seam is two selections). */
export const MAX_LASSO_LON_SPAN = 180_000_000;

/** The µdeg box a ring's points span. */
function ringBox(points: readonly LatLon[]): UBox {
    let minLat = Infinity;
    let minLon = Infinity;
    let maxLat = -Infinity;
    let maxLon = -Infinity;
    for (const p of points) {
        minLat = Math.min(minLat, p.lat);
        minLon = Math.min(minLon, p.lon);
        maxLat = Math.max(maxLat, p.lat);
        maxLon = Math.max(maxLon, p.lon);
    }
    return { minLat, minLon, maxLat, maxLon };
}

/** Even-odd containment of a point in the ring (closed implicitly: the segment
 *  from the last point back to the first counts). The same rule as the map's
 *  even-odd fill, so what this selects is what the drawing showed. */
function pointInRing(lat: number, lon: number, points: readonly LatLon[]): boolean {
    let inside = false;
    for (let k = 0; k < points.length; k++) {
        const a = points[k];
        const b = points[(k + 1) % points.length];
        if (a.lat > lat === b.lat > lat) continue;
        const crossLon = a.lon + ((lat - a.lat) / (b.lat - a.lat)) * (b.lon - a.lon);
        if (lon < crossLon) inside = !inside;
    }
    return inside;
}

/** Whether the segment `a→b` overlaps the filled box — Liang–Barsky: clip the
 *  segment's parameter interval against each slab and see whether anything is
 *  left. A segment wholly inside the box clips to itself and overlaps. */
function segOverlapsBox(a: LatLon, b: LatLon, box: UBox): boolean {
    const dLat = b.lat - a.lat;
    const dLon = b.lon - a.lon;
    let t0 = 0;
    let t1 = 1;
    const clip = (p: number, q: number): boolean => {
        if (p === 0) return q >= 0;
        const r = q / p;
        if (p < 0) {
            if (r > t1) return false;
            if (r > t0) t0 = r;
        } else {
            if (r < t0) return false;
            if (r < t1) t1 = r;
        }
        return true;
    };
    return (
        clip(-dLat, a.lat - box.minLat) &&
        clip(dLat, box.maxLat - a.lat) &&
        clip(-dLon, a.lon - box.minLon) &&
        clip(dLon, box.maxLon - a.lon)
    );
}

/**
 * Every cell of size `2^log2` whose square intersects the closed ring, in
 * ascending `(i, j)` order.
 *
 * The ring closes itself: the last point connects back to the first, so callers
 * hand over the gesture's points as drawn. Fewer than three points select
 * nothing — a degenerate lasso has no inside, and returning the cells under a
 * stray click would turn a slip into a part.
 *
 * Throws a {@link GridError} for a ring spanning more than half the world in
 * longitude ({@link MAX_LASSO_LON_SPAN}), and for a ring whose candidate box
 * exceeds what `cellsIntersecting` will enumerate.
 */
export function lassoCells(log2: number, points: readonly LatLon[]): CellId[] {
    if (points.length < 3) return [];
    const box = ringBox(points);
    if (box.maxLon - box.minLon > MAX_LASSO_LON_SPAN) {
        throw new GridError(
            `this lasso spans ${((box.maxLon - box.minLon) / 1e6).toFixed(1)}° of longitude — the cell grid does ` +
                "not wrap at the antimeridian, so a ring crossing it is two selections and two maps",
        );
    }
    const candidates = cellsIntersecting(log2, box);
    return candidates.filter((cell) => {
        const square = cellSquare(cell);
        for (let k = 0; k < points.length; k++) {
            if (segOverlapsBox(points[k], points[(k + 1) % points.length], square)) return true;
        }
        // No edge touches the square, so it is wholly inside or wholly outside
        // the ring — any one corner answers for all four.
        return pointInRing(square.minLat, square.minLon, points);
    });
}
