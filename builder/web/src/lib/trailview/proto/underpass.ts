// PROTOTYPE (crossing look study, never merged): the road under the railway at A1 as one plain box. The four tunnel
// ways (two carriageways, two cycleways) get one floor each in it, at their own heights; the box has two side walls, a
// flat roof and at each edge of the railway's body an end face with one opening. The ground plan draws the ways up to
// the box and the railway over it; the heights are set by hand (proto/heights.ts).
import { LIFT_M, type Line } from '../terrain/bins';
import { BORE, MeshBuilder, PAINT, paintOf, rgb } from '../structures/mesh';
import { A1, A1_M, A1_RAIL, A1_ROAD } from './heights';
import { VARIANT } from './places';

/** The way through the box in world axes (x east, y south): bearing 78°, across the tracks. */
const D: [number, number] = [0.978, -0.209];
/** The end faces stand this far from the middle: at the edges of the railway's body. The box's half-width for the shaders. */
export const BOX_HALF_M = 10.3, BOX_WIDE_M = 8.35;
/** The floors reach this far past the end faces, and the shaders end the plan's ground there: the plan builds a wall across each tunnel way's end, against the railway's bank. */
const SILL_M = 1.6;
/** The roof's underside: 4.5 m over the higher carriageway end (448.96 m in the bake); the roof is 0.6 m thick. */
export const ROOF_Z = 448.96 + 4.5 + A1_ROAD, ROOF_M = 0.6;
/** The end faces' top: a quarter of a metre over the rails (454.33 m in the bake). */
const TOP_Z = 454.33 + A1_RAIL + 0.25, WING_M = 6, FACE_M = 0.5;
const CONCRETE = rgb('#a9a69d'), COPING = rgb('#bab7ae'), LINING = rgb('#8a877f', BORE), KERB = rgb('#8d8b86', BORE), RAIL = rgb('#3c4046');

type V3 = [number, number, number];

/** Whether a tunnel line is one of the underpass's ways: both its ends lie at the box. */
export function isUnderpass(xy: number[]): boolean {
    const near = (x: number, y: number) => Math.hypot(x - A1[0], y - A1[1]) * A1_M < 22;
    return !!VARIANT && near(xy[0], xy[1]) && near(xy[xy.length - 2], xy[xy.length - 1]);
}

/** The metres along the box and across it (to the right of the way east: south) of a world point. */
const local = (x: number, y: number): [number, number] => {
    const px = (x - A1[0]) * A1_M, py = (y - A1[1]) * A1_M;
    return [px * D[0] + py * D[1], -D[1] * px + D[0] * py];
};
const place = (s: number, a: number): [number, number] => [A1[0] + (D[0] * s - D[1] * a) / A1_M, A1[1] + (D[1] * s + D[0] * a) / A1_M];

/** The box and its four floors. */
export function underpassMesh(b: MeshBuilder, lines: Line[]): void {
    const right: V3 = [-D[1], -D[0], 0], into: V3 = [D[0], -D[1], 0], up: V3 = [0, 0, 1], down: V3 = [0, 0, -1];
    const v = (s: number, a: number, z: number, colour: number) => b.vertex(...place(s, a), z, colour);
    // Each way as a straight strip from end face to end face: its middle, its level and its half-width at each end.
    const ways = lines.map(line => {
        const n = line.z.length, [s0, a0] = local(line.xy[0], line.xy[1]), [s1, a1] = local(line.xy[2 * n - 2], line.xy[2 * n - 1]);
        const at = (s: number) => { const u = (s - s0) / (s1 - s0); return { a: a0 + (a1 - a0) * u, z: line.z[0] + (line.z[n - 1] - line.z[0]) * u }; };
        return { line, h: line.h, west: at(-BOX_HALF_M - SILL_M), east: at(BOX_HALF_M + SILL_M), forward: s1 > s0 };
    }).sort((p, q) => p.west.a - q.west.a);
    const ends = [-BOX_HALF_M - SILL_M, BOX_HALF_M + SILL_M], faces = [-BOX_HALF_M, BOX_HALF_M], lowest = Math.min(...ways.flatMap(w => [w.west.z, w.east.z]));
    // The side walls stand where the shaders end the ground: the ways' own edges lie within.
    const low = -BOX_WIDE_M, high = BOX_WIDE_M;
    ways.forEach((w, i) => {
        // The way's own surface.
        const paint = paintOf(w.line, PAINT | BORE), sign = w.forward ? 1 : -1;
        const at = (end: typeof w.west, s: number, across: number) => b.vertex(...place(s, end.a + across), end.z + LIFT_M, paint, -sign * across, w.h, sign * s);
        b.quad([at(w.west, ends[0], -w.h), at(w.west, ends[0], w.h), at(w.east, ends[1], w.h), at(w.east, ends[1], -w.h)], up);
        // To each side: where the neighbour's edge leaves a gap (or the wall stands off), a strip at the way's level to the gap's middle
        // (or to the wall); where the way lies over its neighbour there, a riser down to it, at the strip's edge or at the way's own.
        for (const side of [-1, 1]) {
            const other = ways[i + side];
            const span = (['west', 'east'] as const).map((end, k) => {
                const inner = w[end].a + side * w.h, far = other ? other[end].a - side * other.h : side < 0 ? low : high, gap = side * (far - inner) > 0;
                return { s: ends[k], inner, outer: gap ? (other ? (inner + far) / 2 : far) : inner, z: w[end].z + LIFT_M, under: other ? other[end].z + LIFT_M : NaN };
            });
            const [p, q] = span;
            if (Math.abs(p.outer - p.inner) > 0.02 || Math.abs(q.outer - q.inner) > 0.02) b.quad([v(p.s, p.inner, p.z, KERB), v(p.s, p.outer, p.z, KERB), v(q.s, q.outer, q.z, KERB), v(q.s, q.inner, q.z, KERB)], up);
            if (other && (p.z > p.under + 0.02 || q.z > q.under + 0.02)) b.quad([v(p.s, p.outer, Math.min(p.under, p.z), KERB), v(q.s, q.outer, Math.min(q.under, q.z), KERB), v(q.s, q.outer, q.z, KERB), v(p.s, p.outer, p.z, KERB)], [side * right[0], side * right[1], 0]);
        }
    });
    // The side walls and the roof, seen from inside; the roof's top, which shows where the railway's bank lies lower.
    for (const [a, side] of [[low, 1], [high, -1]]) b.quad([v(faces[0], a, lowest - 0.3, LINING), v(faces[1], a, lowest - 0.3, LINING), v(faces[1], a, ROOF_Z, LINING), v(faces[0], a, ROOF_Z, LINING)], [side * right[0], side * right[1], 0]);
    b.quad([v(faces[0], low, ROOF_Z, LINING), v(faces[0], high, ROOF_Z, LINING), v(faces[1], high, ROOF_Z, LINING), v(faces[1], low, ROOF_Z, LINING)], down);
    // Over the roof the railway's body is whole. Between the ballast and each end face the plan's bank lies lower than the body: there
    // the fill behind the end face shows, in the ground's look, just under the rails' level.
    const fill = (s: number, a: number) => b.vertex(...place(s, a), TOP_Z - 0.37, CONCRETE, 0, -15, -2e9);
    b.quad([fill(faces[0], low), fill(faces[0], high), fill(faces[1], high), fill(faces[1], low)], up);
    // Under each way's end a sill down to the lowest floor: the ground before the box may lie lower than a raised way.
    for (const w of ways) for (const [k, end] of [w.west, w.east].entries()) {
        const s = ends[k], out: V3 = [(k ? 1 : -1) * into[0], (k ? 1 : -1) * into[1], 0];
        b.quad([v(s, end.a - w.h - 0.4, lowest - 0.3, KERB), v(s, end.a + w.h + 0.4, lowest - 0.3, KERB), v(s, end.a + w.h + 0.4, end.z, KERB), v(s, end.a - w.h - 0.4, end.z, KERB)], out);
    }
    // The end faces: over the opening from the roof's underside to the top, beside it two wings from under the floor; a coping; a railing.
    for (const end of [-1, 1]) {
        const s = end * BOX_HALF_M, back = end * (BOX_HALF_M - FACE_M), out: V3 = [end * into[0], end * into[1], 0], l = low - WING_M, r = high + WING_M;
        const face = (a0: number, a1: number, z0: number) => b.quad([v(s, a0, z0, CONCRETE), v(s, a1, z0, CONCRETE), v(s, a1, TOP_Z, CONCRETE), v(s, a0, TOP_Z, CONCRETE)], out);
        face(low, high, ROOF_Z);
        face(l, low, lowest - 1);
        face(high, r, lowest - 1);
        // The opening's underside in the face's thickness is the roof; the wings' ends and the face's back stand in the bank.
        b.quad([v(s, l, TOP_Z, COPING), v(s, r, TOP_Z, COPING), v(back, r, TOP_Z, COPING), v(back, l, TOP_Z, COPING)], up);
        b.quad([v(back, l, TOP_Z - 1.2, CONCRETE), v(back, r, TOP_Z - 1.2, CONCRETE), v(back, r, TOP_Z, CONCRETE), v(back, l, TOP_Z, CONCRETE)], [-out[0], -out[1], 0]);
        for (const a of [l, r]) b.quad([v(s, a, TOP_Z - 1.2, CONCRETE), v(back, a, TOP_Z - 1.2, CONCRETE), v(back, a, TOP_Z, CONCRETE), v(s, a, TOP_Z, CONCRETE)], [(a === l ? -1 : 1) * right[0], (a === l ? -1 : 1) * right[1], 0]);
        // The railing: posts every 2 m and two rails.
        const bar = (a0: number, a1: number, z0: number, z1: number) => {
            const quad = [v(s - end * 0.2, a0, TOP_Z + z0, RAIL), v(s - end * 0.2, a1, TOP_Z + z0, RAIL), v(s - end * 0.2, a1, TOP_Z + z1, RAIL), v(s - end * 0.2, a0, TOP_Z + z1, RAIL)] as [number, number, number, number];
            b.quad(quad, out);
            b.quad([quad[1], quad[0], quad[3], quad[2]], [-out[0], -out[1], 0]);
        };
        bar(l, r, 0.99, 1.05);
        bar(l, r, 0.5, 0.55);
        for (let a = l; a <= r + 1e-6; a += (r - l) / Math.round((r - l) / 2)) bar(a - 0.03, a + 0.03, 0, 1.05);
    }
}

/** The box in WGSL: whether the ground at `u` units about the anchor and height `h` lies in it, under the roof's top. */
export function boxWgsl(): string {
    return `
fn proto_box(u: vec2f, h: f32) -> bool {
    let q = proto_local(u, vec2i(${A1[0]}, ${A1[1]}), vec2f(${D[0]}, ${D[1]}), ${A1_M});
    return abs(q.x) < ${BOX_HALF_M + SILL_M} && abs(q.y) < ${BOX_WIDE_M} && h < ${(ROOF_Z + ROOF_M - 0.05).toFixed(3)};
}
`;
}
