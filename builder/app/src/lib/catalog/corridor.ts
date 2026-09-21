// Buffering a route into cells: which cells of a band lie within `radius` of a
// polyline, at one global corridor width for every route in the map.
//
// It is a distance-from-polyline test evaluated against whole cell squares, not a
// polygon buffer: nothing here builds an offset ring, so there is no geometry
// library, no dependency and no arc-tolerance parameter to get wrong. The question
// a cell asks is binary — is any part of my square within R of the route? — and the
// answer is a segment-to-rectangle distance, elementary and exact in its plane.
//
// The distance is computed in a local equirectangular plane per cell, with 111 320
// the metres-per-degree the packer and the firmware already use. Over the ~30 km a
// cell spans that is accurate to well under a percent. What the error buys or costs
// is one *whole cell*, and the coverage outline the user sees before downloading is
// drawn from the same cell set this returns, so there is no silent failure mode.
//
// Latitude is *not* corrected per point of the polyline, only per cell: a route
// point that matters to a cell's answer is by definition within the corridor width
// of it, so it shares the cell's latitude to within a fraction of a degree.

import { cellsIntersecting, cellSize, cellSquare, GridError, type CellId, type UBox } from "./grid";

/** A coordinate in integer microdegrees, `lat, lon` — the catalog's order. */
export interface LatLon {
    lat: number;
    lon: number;
}

/** Metres per degree of latitude. The same value `host/obc-pack/src/geom.rs` and
 *  `obc-map-scene` use, so a metre means one thing across the project. */
export const M_PER_DEG = 111_320;

/** Metres per µdeg of latitude. */
const M_PER_UDEG = M_PER_DEG / 1e6;

/** cos(latitude) never divides by less than this. At ±85° the grid's cells are
 *  narrower than a corridor is wide; the clamp keeps the *candidate* box generous
 *  rather than infinite, and the exact test below still decides. */
const MIN_COS = Math.cos((85 * Math.PI) / 180);

/**
 * The east–west scale factor at a latitude, and — the load-bearing part — a
 * function that is **non-increasing in `|lat|` everywhere**.
 *
 * The latitude is clamped to ±90° before the cosine, so a coordinate outside the
 * geographic domain cannot make the cosine climb back towards 1 and hand out a
 * *smaller* padding than a cell nearer the equator got. The prefilter below is only
 * provably generous because this function never goes back up.
 */
function cosLat(latUdeg: number): number {
    const clamped = Math.min(Math.max(latUdeg, -90_000_000), 90_000_000);
    return Math.max(Math.cos((clamped / 1e6) * (Math.PI / 180)), MIN_COS);
}

/** Squared distance from a point to an axis-aligned rectangle; zero inside. */
function pointRectDist2(px: number, py: number, r: PlaneRect): number {
    const dx = Math.max(r.minX - px, 0, px - r.maxX);
    const dy = Math.max(r.minY - py, 0, py - r.maxY);
    return dx * dx + dy * dy;
}

/** Squared distance from a point to a segment. */
function pointSegDist2(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
    const vx = bx - ax;
    const vy = by - ay;
    const len2 = vx * vx + vy * vy;
    let t = 0;
    if (len2 > 0) t = Math.min(1, Math.max(0, ((px - ax) * vx + (py - ay) * vy) / len2));
    const dx = px - (ax + t * vx);
    const dy = py - (ay + t * vy);
    return dx * dx + dy * dy;
}

interface PlaneRect {
    minX: number;
    minY: number;
    maxX: number;
    maxY: number;
}

/**
 * Smallest distance between a segment and an axis-aligned rectangle.
 *
 * Zero when they touch; otherwise the closest approach is realised either at a
 * segment endpoint or at a rectangle corner — there is no third case for convex
 * shapes, which is why this needs no iteration.
 */
function segRectDist2(ax: number, ay: number, bx: number, by: number, r: PlaneRect): number {
    let best = Math.min(pointRectDist2(ax, ay, r), pointRectDist2(bx, by, r));
    if (best === 0) return 0;
    const corners: [number, number][] = [
        [r.minX, r.minY],
        [r.maxX, r.minY],
        [r.maxX, r.maxY],
        [r.minX, r.maxY],
    ];
    for (const [cx, cy] of corners) {
        best = Math.min(best, pointSegDist2(cx, cy, ax, ay, bx, by));
        if (best === 0) return 0;
    }
    return best;
}

/** The µdeg box a polyline's points span. */
function polylineBox(points: readonly LatLon[]): UBox {
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

/** How far, in µdeg, a cell can sit from the route and still be accepted. */
interface Padding {
    dLat: number;
    dLon: number;
}

/**
 * The padding the prefilter must use to be **provably at least** the exact test's
 * acceptance region.
 *
 * The exact test measures a cell's distance in a plane scaled at *that cell's
 * centre* latitude, so a cell accepts a route point up to
 * `R / (M_PER_UDEG · cosLat(φ_cell))` µdeg away in longitude. Padding by the
 * *route's* latitude is smaller than that whenever the cell sits poleward of the
 * route, and a candidate the prefilter drops is never tested, never selected and
 * never reported as missing: a silent hole in the corridor.
 *
 * So the padding is computed at the worst latitude any *candidate cell centre* can
 * have: the route's own extent, grown by the exact latitude padding, then by a whole
 * cell. Because {@link cosLat} never increases with `|lat|`, the cosine there is
 * ≤ the cosine at every candidate's centre, so `dLon` is ≥ every candidate's own
 * requirement.
 */
function paddingFor(box: UBox, radiusM: number, log2: number): Padding {
    const dLat = Math.ceil(radiusM / M_PER_UDEG);
    const worstLat =
        Math.max(Math.abs(box.minLat - dLat), Math.abs(box.maxLat + dLat)) + cellSize(log2);
    return { dLat, dLon: Math.ceil(dLat / cosLat(worstLat)) };
}

/** A box grown by a padding, in µdeg. Only ever selects candidates for the
 *  exact test. */
function grow(box: UBox, pad: Padding): UBox {
    return {
        minLat: box.minLat - pad.dLat,
        minLon: box.minLon - pad.dLon,
        maxLat: box.maxLat + pad.dLat,
        maxLon: box.maxLon + pad.dLon,
    };
}

function boxesIntersect(a: UBox, b: UBox): boolean {
    return a.minLat <= b.maxLat && b.minLat <= a.maxLat && a.minLon <= b.maxLon && b.minLon <= a.maxLon;
}

/**
 * The widest a route may reach in longitude, µdeg: half the world.
 *
 * Past this a polyline is a route written across the antimeridian — the seam the
 * grid deliberately does not wrap over. Buffering it as one box would silently
 * select every cell from one side of the world to the other and price them. Two
 * sides of the seam are two selections, so this refuses rather than guessing.
 */
export const MAX_CORRIDOR_LON_SPAN = 180_000_000;

/**
 * Every cell of size `2^log2` within `radiusM` of the polyline, in ascending
 * `(i, j)` order.
 *
 * A single-point "polyline" buffers to a disc, which is what a one-point GPX
 * track means. A zero or negative radius still selects the cells the line
 * crosses — a corridor of no width is still a corridor.
 *
 * Throws a {@link GridError} for a route spanning more than half the world in
 * longitude ({@link MAX_CORRIDOR_LON_SPAN}), and for a corridor whose candidate
 * box exceeds what `cellsIntersecting` will enumerate.
 */
export function corridorCells(log2: number, points: readonly LatLon[], radiusM: number): CellId[] {
    if (points.length === 0) return [];
    const radius = Math.max(radiusM, 0);
    const box = polylineBox(points);
    if (box.maxLon - box.minLon > MAX_CORRIDOR_LON_SPAN) {
        throw new GridError(
            `this route spans ${((box.maxLon - box.minLon) / 1e6).toFixed(1)}° of longitude — the cell grid does ` +
                "not wrap at the antimeridian, so a route crossing it is two selections and two maps",
        );
    }
    // One padding for the whole run, computed at the worst latitude any candidate
    // can reach — see `paddingFor`. A per-segment latitude here would reintroduce
    // exactly the under-selection it exists to prevent.
    const pad = paddingFor(box, radius, log2);
    const candidates = cellsIntersecting(log2, grow(box, pad));
    if (candidates.length === 0) return [];

    // One padded box per segment, in µdeg, so a cell nowhere near a segment costs
    // four comparisons instead of a projection. A long route over a wide selection
    // is otherwise O(cells × points) of pointless arithmetic.
    const segments: { a: LatLon; b: LatLon }[] =
        points.length === 1 ? [{ a: points[0], b: points[0] }] : [];
    for (let k = 1; k < points.length; k++) segments.push({ a: points[k - 1], b: points[k] });
    const padded = segments.map((s) =>
        grow(
            {
                minLat: Math.min(s.a.lat, s.b.lat),
                minLon: Math.min(s.a.lon, s.b.lon),
                maxLat: Math.max(s.a.lat, s.b.lat),
                maxLon: Math.max(s.a.lon, s.b.lon),
            },
            pad,
        ),
    );

    const r2 = radius * radius;
    return candidates.filter((cell) => {
        const square = cellSquare(cell);
        // Half-open squares meet the closed boxes above at their shared edge; the one
        // µdeg of slack is far below the metre this is accurate to, and it errs toward
        // including a cell.
        const lat0 = (square.minLat + square.maxLat) / 2;
        const lon0 = (square.minLon + square.maxLon) / 2;
        const sx = M_PER_UDEG * cosLat(lat0);
        const sy = M_PER_UDEG;
        const rect: PlaneRect = {
            minX: (square.minLon - lon0) * sx,
            minY: (square.minLat - lat0) * sy,
            maxX: (square.maxLon - lon0) * sx,
            maxY: (square.maxLat - lat0) * sy,
        };
        for (let k = 0; k < segments.length; k++) {
            if (!boxesIntersect(square, padded[k])) continue;
            const { a, b } = segments[k];
            const d2 = segRectDist2(
                (a.lon - lon0) * sx,
                (a.lat - lat0) * sy,
                (b.lon - lon0) * sx,
                (b.lat - lat0) * sy,
                rect,
            );
            if (d2 <= r2) return true;
        }
        return false;
    });
}
