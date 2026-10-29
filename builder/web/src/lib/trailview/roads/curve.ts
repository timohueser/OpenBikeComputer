// PROTOTYPE road system (a study; never merges). The smooth curve of a fitted line and its design: a C1
// cubic Hermite curve through the line's points with the fit's tangents and grades, and the banking and
// the hairpin widening of a road class by its design speed.

/** Per road class, the design speed in km/h and the most banking: main roads bank up to 7 %, minor roads less, tracks and paths not. */
const DESIGN: Record<string, [number, number]> = {
    motorway: [100, 0.07], motorway_link: [60, 0.07], trunk: [80, 0.07], trunk_link: [50, 0.06], primary: [70, 0.07], primary_link: [45, 0.06],
    secondary: [60, 0.065], secondary_link: [40, 0.06], tertiary: [50, 0.06], tertiary_link: [40, 0.05], unclassified: [40, 0.05],
    residential: [30, 0.03], living_street: [15, 0], service: [25, 0.025], road: [30, 0.03],
};
/** Of the side friction, the share that the banking takes: e = V² / (127 R) · BANK_SHARE. */
const BANK_SHARE = 0.5;
/** A bend tighter than WIDEN_R_M widens on its inside, up to WIDEN_M at WIDEN_FULL_R_M. */
const WIDEN_R_M = 25, WIDEN_FULL_R_M = 10, WIDEN_M = 2;
const WIDENS = ['motorway_link', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified'];

/** The banking of a class in a bend of signed curvature k (per metre, positive to the left): the rise per metre to the left, so its inside lies lower. */
export function bankOf(name: string, k: number): number {
    const [speed, most] = DESIGN[name] ?? [0, 0];
    if (!most || !k) return 0;
    return -Math.sign(k) * Math.min(most, (speed * speed * Math.abs(k) / 127) * BANK_SHARE);
}

/** The widening of a class's carriageway on the inside of a bend of signed curvature k, in metres. */
export function widenOf(name: string, k: number): number {
    if (!WIDENS.includes(name) || !k) return 0;
    const r = 1 / Math.abs(k);
    return WIDEN_M * Math.min(1, Math.max(0, (WIDEN_R_M - r) / (WIDEN_R_M - WIDEN_FULL_R_M)));
}

/** A running mean of `v` over ±`half` metres of `s` (sorted), with weights tapering linearly: a triangle filter. */
export function smoothAlong(v: ArrayLike<number>, s: ArrayLike<number>, half: number): Float64Array {
    const n = v.length, out = new Float64Array(n);
    for (let i = 0, lo = 0, hi = 0; i < n; i++) {
        while (s[lo] < s[i] - half) lo++;
        while (hi + 1 < n && s[hi + 1] <= s[i] + half) hi++;
        let sum = 0, weight = 0;
        for (let j = lo; j <= hi; j++) {
            const w = 1 - Math.abs(s[j] - s[i]) / half;
            sum += w * v[j];
            weight += w;
        }
        out[i] = weight ? sum / weight : v[i];
    }
    return out;
}

/** The derivative at each sample of `f` over `s` of the parabola through it and its neighbours (Bessel), one-sided at the ends. */
export function bessel(f: ArrayLike<number>, s: ArrayLike<number>): Float64Array {
    const n = f.length, out = new Float64Array(n);
    for (let i = 0; i < n; i++) {
        if (n < 2) break;
        if (i === 0) out[i] = (f[1] - f[0]) / Math.max(s[1] - s[0], 1e-9);
        else if (i === n - 1) out[i] = (f[i] - f[i - 1]) / Math.max(s[i] - s[i - 1], 1e-9);
        else {
            const h0 = Math.max(s[i] - s[i - 1], 1e-9), h1 = Math.max(s[i + 1] - s[i], 1e-9), d0 = (f[i] - f[i - 1]) / h0, d1 = (f[i + 1] - f[i]) / h1;
            out[i] = (h1 * d0 + h0 * d1) / (h0 + h1);
        }
    }
    return out;
}

/**
 * The cubic Hermite segment from p0 to p1 with unit tangents t0, t1 (plan) and grades g0, g1 (rise per
 * metre): the point, its unit tangent and its height at u in 0..1. The tangents scale by the chord, so a
 * straight segment stays straight and evenly spaced.
 */
export function hermite(p0: [number, number], p1: [number, number], t0: [number, number], t1: [number, number], z0: number, z1: number, g0: number, g1: number, metres: number, u: number) {
    const chord = Math.hypot(p1[0] - p0[0], p1[1] - p0[1]);
    const u2 = u * u, u3 = u2 * u, h00 = 2 * u3 - 3 * u2 + 1, h10 = u3 - 2 * u2 + u, h01 = -2 * u3 + 3 * u2, h11 = u3 - u2;
    const d00 = 6 * u2 - 6 * u, d10 = 3 * u2 - 4 * u + 1, d01 = -6 * u2 + 6 * u, d11 = 3 * u2 - 2 * u;
    const x = h00 * p0[0] + h10 * t0[0] * chord + h01 * p1[0] + h11 * t1[0] * chord;
    const y = h00 * p0[1] + h10 * t0[1] * chord + h01 * p1[1] + h11 * t1[1] * chord;
    const dx = d00 * p0[0] + d10 * t0[0] * chord + d01 * p1[0] + d11 * t1[0] * chord;
    const dy = d00 * p0[1] + d10 * t0[1] * chord + d01 * p1[1] + d11 * t1[1] * chord;
    const z = h00 * z0 + h10 * g0 * metres + h01 * z1 + h11 * g1 * metres;
    const l = Math.hypot(dx, dy) || 1;
    return { x, y, z, tx: dx / l, ty: dy / l };
}
