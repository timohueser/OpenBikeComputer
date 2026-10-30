// PROTOTYPE road system (a study; never merges). The corridors of a node: each road, track, path and
// railway on the ground swept as a mesh along its stations: the carriageway, its sides (shoulder or kerb
// and sidewalk, ballast) and the band, rows of the stamped ground past them that end on the drawn mesh.
// The terrain cuts a hole under the near corridors (hole.wgsl), so the band is the ground there. A segment
// belongs to the node that holds its first station; a dead end gets a round cap.
//
// The band's heights are offsets over the ground, known only on the GPU, so faces are wound here from
// their plan (seen from above, counter-clockwise), not from their heights.
import { junctions, RAIL, type Line } from '../terrain/bins';
import { NO_SLEEPERS, type MeshBuilder } from '../structures/mesh';
import type { Owns } from '../structures/node';
import { carriageway, corridorLine, MAT, sideOf, type Part } from './section';
import { CUT, junctionPlan, type JStation } from './junctions';
import type { Station } from './stations';

/** Vertex flags in the colour's alpha that mark a corridor vertex: VOID and INSIDE of mesh.ts, which no other face has together. */
const CORRIDOR = 12;
/** Steps of a dead end's half circle. */
const CAP_STEPS = 10;

/** The colour word of a corridor vertex: the material, a parameter (the surface code), the flags and the band's priority. */
export const corridorColour = (mat: number, param: number, flags = 0, prio = 0) => (mat | (param << 8) | ((flags | (prio << 4)) << 16) | (CORRIDOR << 24)) >>> 0;

/** Lifts that keep the carriageways of two lines at a junction apart: the better class higher. */
export const classLift = (line: Line, id: number) => Math.max(0, 30 - line.profile) * 0.001 + (id % 2) * 0.0005;
/** The band of the better class draws over the other where two bands overlap. */
const bandPrio = (line: Line) => Math.max(0, Math.min(7, 7 - (line.profile >> 2)));

/** The vertices of a line made so far, by station, place across, height, material and flags: neighbouring quads share them. */
export interface Ctx { b: MeshBuilder; line: Line; lift: number; prio: number; made: Map<string, V> }
/** A vertex and its plan position in world units. */
export interface V { i: number; x: number; y: number }

export function corridors(b: MeshBuilder, lines: Line[], owns: Owns): void {
    const shared = junctions(lines), plan = junctionPlan(lines), ctxs = new Map<Line, Ctx>();
    lines.forEach((line, id) => {
        if (!corridorLine(line) || line.z.length < 2) return;
        const ctx: Ctx = { b, line, lift: classLift(line, id), prio: bandPrio(line), made: new Map() }, stations = plan.stations(line);
        ctxs.set(line, ctx);
        for (let j = 0; j + 1 < stations.length; j++) {
            if (owns(stations[j].x, stations[j].y)) sweep(ctx, stations[j], stations[j + 1]);
        }
        const n = line.z.length;
        for (const [end, s, dir] of [[0, stations[0], -1], [n - 1, stations[stations.length - 1], 1]] as const) {
            if (!shared.has(`${line.xy[2 * end]},${line.xy[2 * end + 1]}`) && owns(s.x, s.y)) cap(ctx, s, dir);
        }
    });
    plan.faces(line => ctxs.get(line), owns);
}

/** A vertex at station s, `a` metres across (left positive), `dz` up, in material `mat`. */
export function vertex(ctx: Ctx, s: Station, a: number, dz: number, mat: number, flags = 0): V {
    const key = s.j === undefined ? undefined : `${s.j}|${a}|${dz}|${mat}|${flags}`, known = key === undefined ? undefined : ctx.made.get(key);
    if (known) return known;
    const { b, line, lift, prio } = ctx, sampled = (flags & 3) !== 0, along = line.kind === RAIL ? (s.sleepers ?? NO_SLEEPERS) : s.along;
    // A sampled height is an offset over the ground; else the station's height, its bank across and the lift.
    const z = sampled ? dz : s.z + dz + s.bank * a + lift, x = s.x + s.ax * a, y = s.y + s.ay * a;
    const made = { i: b.vertex(x, y, z, corridorColour(mat, line.code, flags, prio), a, line.h, along), x, y };
    if (key !== undefined) ctx.made.set(key, made);
    return made;
}

/**
 * A quad seen from above, counter-clockwise. A vertical face (no area in plan) stands on side `side` of its
 * line (1 left, -1 right) and faces the line's middle, as a kerb faces its road.
 */
export function quad(b: MeshBuilder, q: [V, V, V, V], side: number): void {
    // World y runs south: the signed area of the plan with y flipped.
    let area = 0;
    for (let k = 0; k < 4; k++) area += q[k].x * -q[(k + 1) % 4].y - q[(k + 1) % 4].x * -q[k].y;
    const ccw = Math.abs(area) > 1e-3 ? area > 0 : side < 0;
    if (ccw) b.face(q[0].i, q[1].i, q[2].i, q[3].i);
    else b.face(q[0].i, q[3].i, q[2].i, q[1].i);
}

/** The faces between stations s and t: the carriageway, then each side out to the end of its band. */
function sweep(ctx: Ctx, s: JStation, t: Station): void {
    const { line } = ctx, road = carriageway(line), cut = s.cut ?? 0;
    if (!(cut & CUT.ROAD)) for (let k = 0; k + 1 < road.length; k++) strip(ctx, s, t, road[k], road[k + 1], 1);
    for (const side of [1, -1] as const) {
        if (cut & (side > 0 ? CUT.LEFT : CUT.RIGHT)) continue;
        let inner: Part = { a: side * line.h, dz: road[0].dz, mat: MAT.PAINT };
        for (const part of sideOf(line, side, s.v)) {
            const outer = { ...part, a: side * part.a };
            strip(ctx, s, t, inner, outer, side);
            inner = outer;
        }
    }
}

/** The quad from part p to part q (signed metres across) between stations s and t, in q's material. */
export function strip(ctx: Ctx, s: Station, t: Station, p: Part, q: Part, side: number): void {
    const v = (st: Station, part: Part) => vertex(ctx, st, part.a, part.dz, q.mat, part.flags ?? 0);
    quad(ctx.b, [v(s, p), v(s, q), v(t, q), v(t, p)], side);
}

/** A dead end's half circle: the carriageway's end as a fan, then the band's rings, so the hole under it is covered. */
function cap(ctx: Ctx, s: Station, dir: number): void {
    const { b, line } = ctx, rings = sideOf(line, 1, s.v), dz = carriageway(line)[0].dz;
    // Ahead past the end, in world units per metre: the left vector turned a quarter.
    const fx = -s.ay * dir, fy = s.ax * dir;
    // The station `r` metres from the end at angle `t` from the left (0) through ahead (π/2) to the right (π).
    const at = (r: number, t: number): Station => ({ ...s, j: undefined, x: s.x + (s.ax * Math.cos(t) + fx * Math.sin(t)) * r, y: s.y + (s.ay * Math.cos(t) + fy * Math.sin(t)) * r });
    const centre = vertex(ctx, s, 0, dz, MAT.PAINT);
    for (let k = 0; k < CAP_STEPS; k++) {
        const t0 = (Math.PI * k) / CAP_STEPS, t1 = (Math.PI * (k + 1)) / CAP_STEPS;
        const e0 = vertex(ctx, at(line.h, t0), 0, dz, MAT.PAINT), e1 = vertex(ctx, at(line.h, t1), 0, dz, MAT.PAINT);
        quad(b, [centre, e0, e1, e1], 1);
        let inner: Part = { a: line.h, dz, mat: MAT.PAINT };
        for (const outer of rings) {
            const v = (tt: number, part: Part) => vertex(ctx, at(part.a, tt), 0, part.dz, outer.mat, part.flags ?? 0);
            quad(b, [v(t0, inner), v(t0, outer), v(t1, outer), v(t1, inner)], 1);
            inner = outer;
        }
    }
}
