// The curve between two vertices of a part with an alignment (specs/trailview-features.md, Alignment).

import type { FeaturePart } from './features';

/** A chord of `polyline` leaves the curve and its heights by about this. It sets how many segments a stamp holds. */
const SAG_M = 0.2;

const unwound = (angle: number) => Math.atan2(Math.sin(angle), Math.cos(angle));
/** The chords that hold a span of `metres` that turns by `bend`, in radians or in rise per metre, within SAG_M. */
const chords = (metres: number, bend: number) => Math.ceil(Math.sqrt(metres * bend / 8 / SAG_M));

/** The part as a polyline of its curve, with its heights; `metres` per tile unit. A part with no alignment comes back as it is. */
export function polyline({ xy, z, plan }: FeaturePart, metres: number): { xy: ArrayLike<number>; z: ArrayLike<number> | null } {
    if (!plan || !z) return { xy, z };
    const px: number[] = [xy[0], xy[1]], pz: number[] = [z[0]];
    for (let v = 1; v < z.length; v++) {
        const [ax, ay, bx, by] = [xy[2 * v - 2], xy[2 * v - 1], xy[2 * v], xy[2 * v + 1]], dx = bx - ax, dy = by - ay;
        const chord = Math.hypot(dx, dy), [t0, t1, g0, g1] = [plan.tangent[v - 1], plan.tangent[v], plan.grade[v - 1], plan.grade[v]];
        const heading = Math.atan2(dy, dx), turn = Math.abs(unwound(t0 - heading)) + Math.abs(unwound(t1 - heading));
        const steps = Math.max(1, chords(chord * metres, turn), chords(chord * metres, Math.abs(g1 - g0)));
        for (let step = 1; step <= steps; step++) {
            const u = step / steps, u2 = u * u, u3 = u2 * u, h00 = 2 * u3 - 3 * u2 + 1, h10 = u3 - 2 * u2 + u, h01 = 3 * u2 - 2 * u3, h11 = u3 - u2;
            px.push(ax + h10 * Math.cos(t0) * chord + h01 * dx + h11 * Math.cos(t1) * chord, ay + h10 * Math.sin(t0) * chord + h01 * dy + h11 * Math.sin(t1) * chord);
            pz.push(h00 * z[v - 1] + h10 * g0 * chord * metres + h01 * z[v] + h11 * g1 * chord * metres);
        }
    }
    return { xy: px, z: pz };
}
