// PROTOTYPE (crossing look study, never merged): the mesh of a mouth: the floor of the open cut, its sides (banks or
// walls), the frame on the portal line, and the ground beside and over the frame. The ground faces stand at the cut
// field or just under the plain ground, whichever is lower (structures.wgsl clamps them), and the terrain draws nothing
// over the field (terrain.wgsl), so the two meet where the cut meets the ground.
import { LIFT_M, type Line } from '../terrain/bins';
import { strokeMetres } from '../roads/stations';
import { BORE, MeshBuilder, PAINT, paintOf, rgb } from '../structures/mesh';
import { fieldAt, floor, place, zoneField, type Mouth, type Zone } from './places';
import { WALL_GROUND } from './walls';

/** A clamped vertex: the ground's look, or its own colour. */
const GROUND_LOOK = -2e9, OWN_LOOK = -3e9;
/** A clamped ground vertex stands this far under the plain ground where the cut does not reach it. */
const UNDER_M = 0.12;
const CONCRETE = rgb('#a9a69d'), CONCRETE_DARK = rgb('#8e8b83'), COPING = rgb('#bab7ae'), SHOULDER = rgb('#77777a'), RAIL = rgb('#3c4046');
/** The frame's depth before the portal line, its reveal behind it, and how deep it stands in the floor. */
const FRAME_M = 0.6, REVEAL_M = 0.4, SINK_M = 0.6;
/** Behind the frame a sleeve of the opening's section, a little wider than the bore, and the floor under it: the bore begins across its own curve, not across the mouth. */
const SLEEVE_M = 3.5, SLEEVE = rgb('#8a877f', BORE);
const STEP_M = 1;

type V3 = [number, number, number];

/** The mesh of a mouth of `line`, whose way into the tunnel runs along the line with `sign` (+1 from its start). */
export function mouthMesh(b: MeshBuilder, k: Mouth, line: Line, sign: 1 | -1): void {
    const right: V3 = [-k.d[1], -k.d[0], 0], into: V3 = [k.d[0], -k.d[1], 0], up: V3 = [0, 0, 1];
    const neg = ([x, y, z]: V3): V3 => [-x, -y, -z], times = ([x, y, z]: V3, f: number): V3 => [x * f, y * f, z * f];
    /** A vertex at absolute height. */
    const fixed = (s: number, a: number, z: number, colour: number) => b.vertex(...place(k, s, a), z, colour);
    /** A vertex at most `under` below the plain ground. */
    const clamped = (s: number, a: number, z: number, look: number, under = UNDER_M, colour = CONCRETE) => b.vertex(...place(k, s, a), z, colour, 0, under, look);
    /** The cut's height in a zone, with the other mouths' cuts. */
    const cut = (zone: Zone, s: number, a: number) => Math.min(zoneField(k, zone, s, a), fieldAt(...place(k, s, a), k));
    const ground = (zone: Zone, s0: number, s1: number, a0: number, a1: number) =>
        b.quad([[s0, a0], [s0, a1], [s1, a1], [s1, a0]].map(([s, a]) => clamped(s, a, cut(zone, s, a), GROUND_LOOK)) as [number, number, number, number], up);

    const rows = (from: number, to: number) => {
        const out = [from];
        for (let s = Math.ceil(from / STEP_M + 1e-6) * STEP_M; s < to - 1e-6; s += STEP_M) out.push(s);
        return [...out, to];
    };
    const front = [...rows(k.s0 - 1, 0), ...rows(0, k.L).slice(1)], behind = k.cap ? rows(k.L, k.L + k.back + 1) : [];
    const columns = (side: 1 | -1) => rows(k.fh + k.thick, side < 0 ? k.gl : k.gr).map(a => side * a);

    // The floor: the way's own surface from the tag end, and a shoulder to the frame's outer edge.
    const paint = paintOf(line, PAINT), stroke = (s: number) => strokeMetres(line, sign > 0 ? s : lengthOf(line) - s);
    const floors = [k.s0 - 0.15, ...front.filter(s => s > k.s0), k.L + SLEEVE_M];
    for (let i = 0; i + 1 < floors.length; i++) {
        // Under the sleeve the floor lies just under the bore's road.
        const [s0, s1] = [floors[i], floors[i + 1]], lift = s0 >= k.L ? LIFT_M - 0.015 : LIFT_M, grade = (floor(k, k.L) - floor(k, k.L - 1)), over = (s: number) => Math.max(s - k.L, 0) * grade;
        const z0 = floor(k, s0) + over(s0) + lift, z1 = floor(k, s1) + over(s1) + lift;
        // Across to the left of the line's own direction, as the bore's road has it.
        const at = (s: number, a: number, z: number) => b.vertex(...place(k, s, a), z, paint, -sign * a, k.h, stroke(s));
        b.quad([at(s0, -k.h, z0), at(s0, k.h, z0), at(s1, k.h, z1), at(s1, -k.h, z1)], up);
        for (const side of [1, -1]) b.quad([fixed(s0, side * k.h, z0, SHOULDER), fixed(s0, side * k.fh, z0, SHOULDER), fixed(s1, side * k.fh, z1, SHOULDER), fixed(s1, side * k.h, z1, SHOULDER)], up);
    }
    for (const side of [1, -1] as const) {
        const cols = columns(side), out = times(right, side);
        // The sides of the cut before the portal line: the bank, and with a wall its face from the floor.
        for (let i = 0; i + 1 < front.length; i++) {
            const [s0, s1] = [front[i], front[i + 1]];
            for (let j = 0; j + 1 < cols.length; j++) ground('side', s0, s1, cols[j], cols[j + 1]);
        }
        if (k.wall > 0) wall(b, k, side, front, out);
        // Behind the portal line: the bank round the end, and the frame's side that stands out of it.
        for (let i = 0; i + 1 < behind.length; i++) {
            const [s0, s1] = [behind[i], behind[i + 1]], a = side * k.fh;
            for (let j = 0; j + 1 < cols.length; j++) ground('cone', s0, s1, cols[j], cols[j + 1]);
            b.quad([clamped(s0, a, cut('cone', s0, a), OWN_LOOK, UNDER_M, CONCRETE_DARK), clamped(s1, a, cut('cone', s1, a), OWN_LOOK, UNDER_M, CONCRETE_DARK),
                clamped(s1, a, cut('over', s1, a), OWN_LOOK, UNDER_M, CONCRETE_DARK), clamped(s0, a, cut('over', s0, a), OWN_LOOK, UNDER_M, CONCRETE_DARK)], out);
        }
        // Without a round end, the end face beside the frame reaches from the cut's side up to the ground.
        if (!k.cap) for (let j = 0; j + 1 < cols.length; j++) {
            const [a0, a1] = [cols[j], cols[j + 1]];
            b.quad([clamped(k.L, a0, cut('side', k.L, a0), OWN_LOOK), clamped(k.L, a1, cut('side', k.L, a1), OWN_LOOK), clamped(k.L, a1, 1e5, OWN_LOOK, -0.05), clamped(k.L, a0, 1e5, OWN_LOOK, -0.05)], neg(into));
        }
    }
    // And over the frame, from its top up to the ground.
    if (!k.cap) b.quad([fixed(k.L, -k.fh, floor(k, k.L) + k.H, CONCRETE), fixed(k.L, k.fh, floor(k, k.L) + k.H, CONCRETE), clamped(k.L, k.fh, 1e5, OWN_LOOK, -0.05), clamped(k.L, -k.fh, 1e5, OWN_LOOK, -0.05)], neg(into));
    // The ground over the bore behind the frame.
    for (let i = 0; i + 1 < behind.length; i++) ground('over', behind[i], behind[i + 1], -k.fh, k.fh);

    // The frame: a ring about the opening before the portal line, its reveal, its sides and its top, and a coping.
    const zL = floor(k, k.L), s0 = k.L - FRAME_M, s1 = k.L + REVEAL_M, c = k.chamfer;
    const opening: [number, number][] = [[-k.oh, 0], [-k.oh, k.hh - c], [-k.oh + c, k.hh], [k.oh - c, k.hh], [k.oh, k.hh - c], [k.oh, 0]];
    const face = (corners: [number, number][], s: number, colour: number, out: V3) => b.quad(corners.map(([a, up]) => fixed(s, a, zL + up, colour)) as [number, number, number, number], out);
    for (const side of [1, -1]) face([[side * k.oh, -SINK_M], [side * k.fh, -SINK_M], [side * k.fh, k.H], [side * k.oh, k.H]], s0, CONCRETE, neg(into));
    for (let i = 1; i + 2 < opening.length; i++) face([opening[i], opening[i + 1], [opening[i + 1][0], k.H], [opening[i][0], k.H]], s0, CONCRETE, neg(into));
    for (let i = 0; i + 1 < opening.length; i++) {
        const [[a0, u0], [a1, u1]] = [opening[i], opening[i + 1]];
        // Into the opening: away from the ring's body.
        const out: V3 = [(u1 - u0) * right[0], (u1 - u0) * right[1], a0 - a1];
        b.quad([fixed(s0, a0, zL + u0, CONCRETE_DARK), fixed(s0, a1, zL + u1, CONCRETE_DARK), fixed(s1, a1, zL + u1, CONCRETE_DARK), fixed(s1, a0, zL + u0, CONCRETE_DARK)], out);
    }
    // The sleeve.
    const wider: [number, number][] = opening.map(([a, u]) => [a + Math.sign(a) * 0.03, u > 0 ? u + 0.03 : u]);
    for (let i = 0; i + 1 < wider.length; i++) {
        const [[a0, u0], [a1, u1]] = [wider[i], wider[i + 1]], out: V3 = [(u1 - u0) * right[0], (u1 - u0) * right[1], a0 - a1], back = k.L + SLEEVE_M, drop = (zL - floor(k, k.L - 1)) * SLEEVE_M;
        b.quad([fixed(k.L, a0, zL + u0, SLEEVE), fixed(k.L, a1, zL + u1, SLEEVE), fixed(back, a1, zL + drop + u1, SLEEVE), fixed(back, a0, zL + drop + u0, SLEEVE)], out);
    }
    for (const side of [1, -1]) b.quad([fixed(s0, side * k.fh, zL - SINK_M, CONCRETE_DARK), fixed(k.L, side * k.fh, zL - SINK_M, CONCRETE_DARK), fixed(k.L, side * k.fh, zL + k.H, CONCRETE_DARK), fixed(s0, side * k.fh, zL + k.H, CONCRETE_DARK)], times(right, side));
    box(b, k, s0 - 0.12, k.L + 0.5, -k.fh - 0.12, k.fh + 0.12, zL + k.H, zL + k.H + 0.22, COPING);
    // The bore's outer shell behind the frame: under the ground wherever the cover holds.
    box(b, k, k.L, k.L + k.back, -k.oh - 0.4, k.oh + 0.4, zL - SINK_M, zL + k.hh + 0.4, CONCRETE_DARK, false);
    if (k.railing) railings(b, k, front);
}

/** A box in a mouth's local metres, without its bottom. */
function box(b: MeshBuilder, k: Mouth, s0: number, s1: number, a0: number, a1: number, z0: number, z1: number, colour: number, front = true): void {
    const right: V3 = [-k.d[1], -k.d[0], 0], into: V3 = [k.d[0], -k.d[1], 0];
    const v = (s: number, a: number, z: number) => b.vertex(...place(k, s, a), z, colour);
    b.quad([v(s0, a0, z1), v(s0, a1, z1), v(s1, a1, z1), v(s1, a0, z1)], [0, 0, 1]);
    if (front) b.quad([v(s0, a0, z0), v(s0, a1, z0), v(s0, a1, z1), v(s0, a0, z1)], [-into[0], -into[1], 0]);
    b.quad([v(s1, a0, z0), v(s1, a1, z0), v(s1, a1, z1), v(s1, a0, z1)], into);
    b.quad([v(s0, a0, z0), v(s1, a0, z0), v(s1, a0, z1), v(s0, a0, z1)], [-right[0], -right[1], 0]);
    b.quad([v(s0, a1, z0), v(s1, a1, z0), v(s1, a1, z1), v(s0, a1, z1)], right);
}

/** A simple railing on the walls' tops beside the cut and across the end face: posts and two rails on the plain ground. */
function railings(b: MeshBuilder, k: Mouth, front: number[]): void {
    const GROUND = 1, HEIGHT = 1.1, at = (s: number, a: number, over: number) => b.vertex(...place(k, s, a), over, RAIL | (GROUND << 24));
    const bar = (p: [number, number], q: [number, number], low: number, high: number) => {
        const quad: [number, number, number, number] = [at(...p, low), at(...q, low), at(...q, high), at(...p, high)];
        b.quad(quad, [-k.d[1], -k.d[0], 0.001]);
        b.quad([quad[1], quad[0], quad[3], quad[2]], [k.d[1], k.d[0], 0.001]);
    };
    const run = (points: [number, number][]) => {
        for (let i = 0; i + 1 < points.length; i++) {
            bar(points[i], points[i + 1], HEIGHT - 0.06, HEIGHT);
            bar(points[i], points[i + 1], 0.5, 0.55);
            const [s, a] = points[i], [t, c] = points[i + 1], l = Math.hypot(t - s, c - a) || 1;
            bar(points[i], [s + (t - s) * 0.05 / l, a + (c - a) * 0.05 / l], 0, HEIGHT);
        }
    };
    const out = k.fh + 0.15;
    for (const side of [1, -1]) run(front.filter(s => s > k.s0 + 0.5).map(s => [s, side * out] as [number, number]));
    run([[k.L + 0.15, -out], [k.L + 0.15, -out / 2], [k.L + 0.15, 0], [k.L + 0.15, out / 2], [k.L + 0.15, out]]);
}

/** The length of a line in metres at its first vertex's scale. */
function lengthOf(line: Line): number {
    let l = 0;
    for (let v = 0; v + 1 < line.z.length; v++) l += Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]);
    return l * 2 * Math.PI * 6378137 / 2 ** 30 / Math.cosh(Math.PI * (1 - 2 * line.xy[1] / 2 ** 30));
}

/**
 * A wall beside the cut's floor on one side, from the floor to a smooth top: over the plain ground by an upstand, at most
 * the wall's height over the floor, and straight between knots every few metres. The ground comes from WALL_GROUND.
 */
function wall(b: MeshBuilder, k: Mouth, side: 1 | -1, front: number[], out: V3): void {
    const table = WALL_GROUND[k.id];
    if (!table) return;
    const UPSTAND_M = 0.25, KNOT_M = 3, LEAST_M = 0.3, list = side < 0 ? table.left : table.right;
    const ground = (s: number) => list[Math.max(0, Math.min(list.length - 1, Math.round(s - table.from)))];
    // A knot holds the highest ground within its reach, so the top never dips under the ground between two knots.
    const knot = (i: number) => Math.max(...[-1.5, -0.75, 0, 0.75, 1.5].map(d => ground(i * KNOT_M + d))) + UPSTAND_M;
    const top = (s: number) => {
        const i = Math.floor(s / KNOT_M), u = s / KNOT_M - i;
        return Math.min(floor(k, s) + k.wall, knot(i) + (knot(i + 1) - knot(i)) * u);
    };
    const end = k.L - FRAME_M, a0 = side * k.fh, a1 = side * (k.fh + k.thick), v = (s: number, a: number, z: number, colour = CONCRETE) => b.vertex(...place(k, s, a), z, colour);
    const stations = [...front.filter(s => s < end), end];
    for (let i = 0; i + 1 < stations.length; i++) {
        const [s, t] = [stations[i], stations[i + 1]], [zs, zt] = [top(s), top(t)], [fs, ft] = [floor(k, s), floor(k, t)];
        if (zs - fs < LEAST_M && zt - ft < LEAST_M) continue;
        b.quad([v(s, a0, fs - 0.3), v(t, a0, ft - 0.3), v(t, a0, zt), v(s, a0, zs)], [-out[0], -out[1], 0]);
        b.quad([v(s, a0, zs, COPING), v(t, a0, zt, COPING), v(t, a1, zt, COPING), v(s, a1, zs, COPING)], [0, 0, 1]);
        b.quad([v(s, a1, zs - 2, CONCRETE_DARK), v(t, a1, zt - 2, CONCRETE_DARK), v(t, a1, zt, CONCRETE_DARK), v(s, a1, zs, CONCRETE_DARK)], out);
    }
}
