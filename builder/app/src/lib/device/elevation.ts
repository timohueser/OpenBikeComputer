/**
 * The preview's elevation profile: distance-along-track on x, elevation on y, as one SVG path —
 * plus the windowing math behind the chart-room preview's zoom (drag a range, scroll to zoom,
 * double-click to reset).
 *
 * Pure geometry, shared by the route preview (points from the wasm read-back) and the ride
 * preview (points from `decodeRideObject`). Distance is equirectangular with a `cos(lat)`
 * correction — the same approximation `library.ts`'s thumbnail uses, and metres-accurate at
 * track scale.
 *
 * One distance axis for everything: {@link cumulativeDistances} runs over **all** points —
 * elevation-less ones included, since they exist on the map polyline — and both the profile and
 * the map's coral/gray split are driven from it. A zoom window is a `[t0, t1]` pair of fractions
 * of the total distance; {@link elevationProfile} redraws the windowed span from the real points
 * (interpolating the elevation at the cut edges), and {@link windowIndexRange} names the point
 * indices the map highlights for the same window.
 */

/** The minimum a profile is drawn from: with one point there is no distance axis at all. */
const MIN_POINTS = 2;

export interface ProfilePoint {
    readonly lat: number;
    readonly lon: number;
    /** Metres; null where the source recorded none. */
    readonly ele: number | null;
}

/** A zoom window over the track: fractions of the total distance, `0 <= t0 < t1 <= 1`. */
export type ProfileWindow = readonly [number, number];

/** The whole track — the reset state, and the only window a profile-less preview ever has. */
export const FULL_WINDOW: ProfileWindow = [0, 1];

/** The narrowest a window may get — 1% of the track, so a zoom can never collapse or invert. */
export const MIN_WINDOW_SPAN = 0.01;

export interface ElevationProfile {
    /** `M … L …` path in a `width` × `height` viewBox, baseline-closed for an area fill. */
    readonly areaPath: string;
    /** The same polyline without the baseline, for the stroke on top. */
    readonly linePath: string;
    /** Extremes of the elevation *within the window*. */
    readonly minEle: number;
    readonly maxEle: number;
    /** The window's bounds and the whole track's length, metres — the axis captions' numbers. */
    readonly startM: number;
    readonly endM: number;
    readonly totalM: number;
}

/**
 * Cumulative ground distance at every point, metres — `result[0] === 0`, one entry per point,
 * elevation-less points included. The shared axis: the profile's x, the map split's cut, and a
 * waypoint tick's position are all fractions of `result[result.length - 1]`.
 */
export function cumulativeDistances(points: readonly ProfilePoint[]): number[] {
    const cum: number[] = points.length ? [0] : [];
    for (let i = 1; i < points.length; i++) {
        cum.push(cum[i - 1] + groundDistanceM(points[i - 1], points[i]));
    }
    return cum;
}

/** Is `window` (effectively) the whole track — the state with no gray on the map, no reset. */
export function isFullWindow([t0, t1]: ProfileWindow): boolean {
    return t0 <= 0 && t1 >= 1;
}

/**
 * Order and clamp a dragged pair of fractions into a valid window: inside `[0, 1]`, at least
 * {@link MIN_WINDOW_SPAN} wide (grown symmetrically around the pair's centre, pushed back inside
 * the track when the centre sits near an end).
 */
export function clampWindow(a: number, b: number): ProfileWindow {
    let t0 = Math.max(0, Math.min(a, b));
    let t1 = Math.min(1, Math.max(a, b));
    if (t1 - t0 < MIN_WINDOW_SPAN) {
        const centre = (t0 + t1) / 2;
        t0 = centre - MIN_WINDOW_SPAN / 2;
        t1 = centre + MIN_WINDOW_SPAN / 2;
        if (t0 < 0) [t0, t1] = [0, MIN_WINDOW_SPAN];
        if (t1 > 1) [t0, t1] = [1 - MIN_WINDOW_SPAN, 1];
    }
    return [t0, t1];
}

/**
 * Scale a window by `factor` (>1 zooms out, <1 zooms in) about `centreT` — an absolute fraction
 * of the track, normally the cursor's position — keeping the centre's relative position in the
 * window so the ground under the cursor stays put. Clamped to `[0, 1]` and the minimum span.
 */
export function zoomWindow(window: ProfileWindow, factor: number, centreT: number): ProfileWindow {
    const [t0, t1] = window;
    const span = t1 - t0;
    const next = Math.min(1, Math.max(MIN_WINDOW_SPAN, span * factor));
    // Where the anchor sits within the window (0 = left edge, 1 = right), kept across the scale.
    const r = span > 0 ? (centreT - t0) / span : 0.5;
    let n0 = centreT - r * next;
    n0 = Math.max(0, Math.min(1 - next, n0));
    return [n0, n0 + next];
}

/**
 * The point indices the map highlights for `window`: `[first, last]` such that the polyline
 * `points[first..=last]` covers the window's distance span (the boundary segments are included
 * whole — a cut mid-segment rounds outward, so the coral never falls short of the window).
 * `[0, points.length - 1]` at the full window.
 */
export function windowIndexRange(cum: readonly number[], window: ProfileWindow): [number, number] {
    const last = cum.length - 1;
    if (last < 1) return [0, Math.max(0, last)];
    const total = cum[last];
    const d0 = window[0] * total;
    const d1 = window[1] * total;
    let first = 0;
    while (first < last && cum[first + 1] <= d0) first++;
    let end = last;
    while (end > 0 && cum[end - 1] >= d1) end--;
    return first <= end ? [first, end] : [end, first];
}

/**
 * Slide the window along the track by `deltaT` (a fraction of the total distance), preserving its
 * span — the Ctrl/⌘-drag pan. Clamped so the window never leaves `[0, 1]` and never resizes.
 */
export function panWindow(window: ProfileWindow, deltaT: number): ProfileWindow {
    const [t0, t1] = window;
    const span = t1 - t0;
    const n0 = Math.max(0, Math.min(1 - span, t0 + deltaT));
    return [n0, n0 + span];
}

/**
 * The `[lat, lon]` at distance `d` metres along the track — interpolated within the segment that
 * contains it, clamped to the ends. The hover cursor's map position: one distance axis
 * ({@link cumulativeDistances}) in, one point on the drawn polyline out. Null on an empty track.
 */
export function pointAtDistance(
    points: readonly ProfilePoint[],
    cum: readonly number[],
    d: number,
): { lat: number; lon: number } | null {
    if (points.length === 0 || cum.length !== points.length) return null;
    if (points.length === 1) return { lat: points[0].lat, lon: points[0].lon };
    const target = Math.max(0, Math.min(d, cum[cum.length - 1]));
    // Binary search for the segment holding `target` — hover runs per animation frame.
    let lo = 0;
    let hi = cum.length - 1;
    while (lo + 1 < hi) {
        const mid = (lo + hi) >> 1;
        if (cum[mid] <= target) lo = mid;
        else hi = mid;
    }
    const span = cum[hi] - cum[lo];
    const t = span > 0 ? (target - cum[lo]) / span : 0;
    return {
        lat: points[lo].lat + (points[hi].lat - points[lo].lat) * t,
        lon: points[lo].lon + (points[hi].lon - points[lo].lon) * t,
    };
}

/**
 * The index of the track point nearest `(lat, lon)`, by the same equirectangular metric the
 * distance axis uses — the reverse hover: map cursor in, position along the track out
 * (`cum[index]`). A plain scan: preview tracks are thousands of points, and one pass per
 * animation frame is nothing. `-1` on an empty track.
 */
export function nearestPointIndex(points: readonly ProfilePoint[], lat: number, lon: number): number {
    const probe: ProfilePoint = { lat, lon, ele: null };
    let best = -1;
    let bestM = Infinity;
    for (let i = 0; i < points.length; i++) {
        const m = groundDistanceM(points[i], probe);
        if (m < bestM) {
            bestM = m;
            best = i;
        }
    }
    return best;
}

/**
 * Build the profile for the window, or null where there is nothing honest to draw — fewer than
 * two usable points in the window, or no elevation anywhere in it. A *flat* track still draws (a
 * flat line is a true statement); a track with no elevation data does not.
 *
 * The x axis is true distance along the whole track ({@link cumulativeDistances}): points without
 * elevation still advance it, they just contribute no sample. Where the window's edge cuts
 * between two samples, the elevation at the cut is interpolated so the drawn span is exactly the
 * window, not the nearest sample inside it.
 */
export function elevationProfile(
    points: readonly ProfilePoint[],
    width: number,
    height: number,
    window: ProfileWindow = FULL_WINDOW,
    pad = 2,
): ElevationProfile | null {
    const cum = cumulativeDistances(points);
    const totalM = cum.length ? cum[cum.length - 1] : 0;
    if (totalM <= 0) return null;

    // The usable series: (distance, elevation) samples over the whole track.
    const samples: Array<{ d: number; ele: number }> = [];
    for (let i = 0; i < points.length; i++) {
        const ele = points[i].ele;
        if (ele !== null) samples.push({ d: cum[i], ele });
    }
    if (samples.length < MIN_POINTS) return null;

    const startM = window[0] * totalM;
    const endM = window[1] * totalM;
    const drawn = windowSamples(samples, startM, endM);
    if (drawn.length < MIN_POINTS) return null;

    let minEle = Infinity;
    let maxEle = -Infinity;
    for (const s of drawn) {
        if (s.ele < minEle) minEle = s.ele;
        if (s.ele > maxEle) maxEle = s.ele;
    }
    // A dead-flat profile gets a nominal band so the line sits mid-box instead of on an edge.
    const span = Math.max(maxEle - minEle, 1);

    const w = width - 2 * pad;
    const h = height - 2 * pad;
    const x = (d: number) => pad + ((d - startM) / (endM - startM)) * w;
    const y = (ele: number) => pad + (1 - (ele - minEle) / span) * h;

    const parts = drawn.map((s, i) => `${i === 0 ? "M" : "L"}${x(s.d).toFixed(1)} ${y(s.ele).toFixed(1)}`);
    const linePath = parts.join(" ");
    const baselineY = (height - pad).toFixed(1);
    const first = drawn[0];
    const last = drawn[drawn.length - 1];
    const areaPath = `${linePath} L${x(last.d).toFixed(1)} ${baselineY} L${x(first.d).toFixed(1)} ${baselineY} Z`;

    return { areaPath, linePath, minEle, maxEle, startM, endM, totalM };
}

/** The samples inside `[d0, d1]`, with interpolated samples at any edge that cuts the series. */
function windowSamples(
    samples: ReadonlyArray<{ d: number; ele: number }>,
    d0: number,
    d1: number,
): Array<{ d: number; ele: number }> {
    const out: Array<{ d: number; ele: number }> = [];
    for (let i = 0; i < samples.length; i++) {
        const s = samples[i];
        if (s.d < d0) {
            const next = samples[i + 1];
            if (next && next.d > d0) out.push({ d: d0, ele: lerp(s, next, d0) });
        } else if (s.d > d1) {
            const prev = samples[i - 1];
            if (prev && prev.d < d1) out.push({ d: d1, ele: lerp(prev, s, d1) });
            break;
        } else {
            out.push(s);
        }
    }
    return out;
}

function lerp(a: { d: number; ele: number }, b: { d: number; ele: number }, d: number): number {
    const t = b.d === a.d ? 0 : (d - a.d) / (b.d - a.d);
    return a.ele + (b.ele - a.ele) * t;
}

/** Equirectangular ground distance in metres — fine at the scale of one track. */
function groundDistanceM(a: ProfilePoint, b: ProfilePoint): number {
    const R = 6_371_000;
    const toRad = Math.PI / 180;
    const dLat = (b.lat - a.lat) * toRad;
    const dLon = (b.lon - a.lon) * toRad * Math.cos(((a.lat + b.lat) / 2) * toRad);
    return Math.sqrt(dLat * dLat + dLon * dLon) * R;
}
