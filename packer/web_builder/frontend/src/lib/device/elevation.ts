/**
 * The preview's elevation profile: distance-along-track on x, elevation on y, as one SVG path.
 *
 * Pure geometry, shared by the route preview (points from the wasm read-back) and the ride
 * preview (points from `decodeRideObject`). Distance is equirectangular with a `cos(lat)`
 * correction — the same approximation `library.ts`'s thumbnail uses, and metres-accurate at
 * track scale.
 */

/** The minimum a profile is drawn from: with one point there is no distance axis at all. */
const MIN_POINTS = 2;

export interface ProfilePoint {
    readonly lat: number;
    readonly lon: number;
    /** Metres; null where the source recorded none. */
    readonly ele: number | null;
}

export interface ElevationProfile {
    /** `M … L …` path in a `width` × `height` viewBox, baseline-closed for an area fill. */
    readonly areaPath: string;
    /** The same polyline without the baseline, for the stroke on top. */
    readonly linePath: string;
    readonly minEle: number;
    readonly maxEle: number;
    readonly distanceM: number;
}

/**
 * Build the profile, or null where there is nothing honest to draw — fewer than two usable
 * points, or no elevation anywhere. A *flat* track still draws (a flat line is a true statement);
 * a track with no elevation data does not.
 */
export function elevationProfile(
    points: readonly ProfilePoint[],
    width: number,
    height: number,
    pad = 2,
): ElevationProfile | null {
    const usable = points.filter((p) => p.ele !== null);
    if (usable.length < MIN_POINTS) return null;

    // Cumulative distance over the usable points. Gaps where elevation was missing collapse —
    // the x axis is "distance along what we can draw", which is the only axis available.
    const xs: number[] = [0];
    let total = 0;
    for (let i = 1; i < usable.length; i++) {
        total += groundDistanceM(usable[i - 1], usable[i]);
        xs.push(total);
    }
    if (total <= 0) return null;

    let minEle = Infinity;
    let maxEle = -Infinity;
    for (const p of usable) {
        const ele = p.ele as number;
        if (ele < minEle) minEle = ele;
        if (ele > maxEle) maxEle = ele;
    }
    // A dead-flat profile gets a nominal band so the line sits mid-box instead of on an edge.
    const span = Math.max(maxEle - minEle, 1);

    const w = width - 2 * pad;
    const h = height - 2 * pad;
    const x = (d: number) => pad + (d / total) * w;
    const y = (ele: number) => pad + (1 - (ele - minEle) / span) * h;

    const parts = usable.map((p, i) => {
        const cmd = i === 0 ? "M" : "L";
        return `${cmd}${x(xs[i]).toFixed(1)} ${y(p.ele as number).toFixed(1)}`;
    });
    const linePath = parts.join(" ");
    const baselineY = (height - pad).toFixed(1);
    const areaPath = `${linePath} L${x(total).toFixed(1)} ${baselineY} L${x(0).toFixed(1)} ${baselineY} Z`;

    return { areaPath, linePath, minEle, maxEle, distanceM: total };
}

/** Equirectangular ground distance in metres — fine at the scale of one track. */
function groundDistanceM(a: ProfilePoint, b: ProfilePoint): number {
    const R = 6_371_000;
    const toRad = Math.PI / 180;
    const dLat = (b.lat - a.lat) * toRad;
    const dLon = (b.lon - a.lon) * toRad * Math.cos(((a.lat + b.lat) / 2) * toRad);
    return Math.sqrt(dLat * dLat + dLon * dLon) * R;
}
