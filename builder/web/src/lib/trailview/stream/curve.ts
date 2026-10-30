// The curve of a part with an alignment, cut into chords (specs/trailview-features.md, Alignment).

import type { FeaturePart } from './features';

/**
 * A chord leaves the curve by about this at the road's edge and in height. These set how many segments a
 * stamp holds, and so what the stamp worker and the structures along a road cost.
 */
const PLAN_SAG_M = 0.05, HEIGHT_SAG_M = 0.03;

/** The alignment per point of a line. */
export interface Plan {
    /** Radians from x to y. */
    tangent: number[];
    /** Rise per metre. */
    grade: number[];
    /** Rise per metre to the left. */
    bank: number[];
    /** Metres that the half-width gains. */
    widen: number[];
}

/** A part as chords of its curve. `anchors` marks the vertices of the part among the points. */
export interface Curve { xy: number[]; z: number[]; anchors: boolean[]; plan: Plan }

const unwound = (angle: number) => Math.atan2(Math.sin(angle), Math.cos(angle));
/** The chords that hold a span of `metres` that bends by `bend`, in radians or in rise per metre, within `sag`. */
const chords = (metres: number, bend: number, sag: number) => Math.ceil(Math.sqrt(metres * bend / 8 / sag));

/** Adds to `out` the plan a share u of the way from point v of `plan` to the next. */
export function addPlan(out: Plan, { tangent, grade, bank, widen }: Plan, v: number, u: number): void {
    const mix = (values: number[]) => values[v] + (values[v + 1] - values[v]) * u;
    out.tangent.push(tangent[v] + unwound(tangent[v + 1] - tangent[v]) * u);
    out.grade.push(mix(grade));
    out.bank.push(mix(bank));
    out.widen.push(mix(widen));
}

/** Points v up to `to` of a plan. */
export const planPart = ({ tangent, grade, bank, widen }: Plan, v: number, to: number): Plan =>
    ({ tangent: tangent.slice(v, to), grade: grade.slice(v, to), bank: bank.slice(v, to), widen: widen.slice(v, to) });

/**
 * The part with an alignment as chords of its curve, in its tile units, for an edge `half` metres beside
 * it: an edge bends over the same angle as the line, along a longer arc. `scale` gives the world units per
 * tile unit and `metres` the metres per tile unit at a y. A point lies on a whole world unit from the
 * vertex before it, and each segment reads the scale at its own middle, so each tile that holds a segment
 * cuts it alike.
 */
export function curveOf({ xy, z, plan }: FeaturePart, half: number, scale: number, metres: (y: number) => number): Curve {
    if (!plan || !z) throw new Error('A curve needs an alignment and heights.');
    const out: Curve = { xy: [xy[0], xy[1]], z: [z[0]], anchors: [true], plan: { tangent: [plan.tangent[0]], grade: [plan.grade[0]], bank: [plan.bank[0]], widen: [plan.widen[0]] } };
    const push = (x: number, y: number, height: number, anchor: boolean, tangent: number, grade: number, bank: number, widen: number) => {
        out.xy.push(x, y);
        out.z.push(height);
        out.anchors.push(anchor);
        out.plan.tangent.push(tangent);
        out.plan.grade.push(grade);
        out.plan.bank.push(bank);
        out.plan.widen.push(widen);
    };
    for (let v = 1; v < z.length; v++) {
        const [ax, ay, bx, by] = [xy[2 * v - 2], xy[2 * v - 1], xy[2 * v], xy[2 * v + 1]], dx = bx - ax, dy = by - ay, [z0, z1] = [z[v - 1], z[v]];
        const chord = Math.hypot(dx, dy), length = chord * metres((ay + by) / 2), [t0, t1, g0, g1] = [plan.tangent[v - 1], plan.tangent[v], plan.grade[v - 1], plan.grade[v]];
        const [px, py, qx, qy] = [Math.cos(t0) * chord, Math.sin(t0) * chord, Math.cos(t1) * chord, Math.sin(t1) * chord];
        const heading = Math.atan2(dy, dx), turn = Math.abs(unwound(t0 - heading)) + Math.abs(unwound(t1 - heading)), rise = (z1 - z0) / (length || 1);
        // Heights of a span that has no height at one end stay unknown.
        const steps = Number.isNaN(rise) ? 1 : Math.max(1, chords(length + half * turn, turn, PLAN_SAG_M), chords(length, Math.abs(g0 - rise) + Math.abs(g1 - rise), HEIGHT_SAG_M));
        for (let step = 1; step < steps; step++) {
            const u = step / steps, u2 = u * u, u3 = u2 * u, h00 = 2 * u3 - 3 * u2 + 1, h10 = u3 - 2 * u2 + u, h01 = 3 * u2 - 2 * u3, h11 = u3 - u2;
            const d10 = 3 * u2 - 4 * u + 1, d01 = 6 * u - 6 * u2, d11 = 3 * u2 - 2 * u;
            push(ax + Math.round(scale * (h10 * px + h01 * dx + h11 * qx)) / scale, ay + Math.round(scale * (h10 * py + h01 * dy + h11 * qy)) / scale,
                h00 * z0 + h10 * g0 * length + h01 * z1 + h11 * g1 * length, false, Math.atan2(d10 * py + d01 * dy + d11 * qy, d10 * px + d01 * dx + d11 * qx),
                d10 * g0 + d01 * rise + d11 * g1, plan.bank[v - 1] + (plan.bank[v] - plan.bank[v - 1]) * u, plan.widen[v - 1] + (plan.widen[v] - plan.widen[v - 1]) * u);
        }
        // The vertex itself: its neighbour may have no height.
        push(bx, by, z1, true, t1, g1, plan.bank[v], plan.widen[v]);
    }
    return out;
}
