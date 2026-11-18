// PROTOTYPE (#3651, never merges): a tunnel end as a ready-made part, made from the bake's numbers (part-store.ts):
// a head wall with a recessed frame round the opening and wings whose tops follow the hill, a collar whose outer
// shell stands out of the hill, the walls of a short cutting (V2), an underpass box with its wing walls, and the free
// tube end with a slanted cut. Plus the boxes in which the terrain and the ground plan draw nothing.
import { metresPerUnit, RAIL_CODE, type Line } from '../terrain/bins';
import { BORE, LAMP, MeshBuilder, PAINT, paintOf, posts, rgb, VOID, type Corner, type Station } from './mesh';
import { drawerOf, fitOf, partAt, type BoxEnd, type Placement } from './part-store';

const T = 0.5, FACE = 0.8, FRAME = 0.45, RECESS = 0.25, SINK = 2, ARCH = 8, SPRING = 0.6, WALL = 0.5, SLAB = 0.5, COPE = 0.08;
type Look = { face: number; frame: number; coping: number; wall: number; shell: number };
const LOOKS: Record<string, Look> = {
    concrete: { face: rgb('#a9a69d'), frame: rgb('#cbc8bf'), coping: rgb('#d2cfc7'), wall: rgb('#a19e95'), shell: rgb('#9a978e') },
    masonry: { face: rgb('#91857a'), frame: rgb('#bdb2a2'), coping: rgb('#c2bbae'), wall: rgb('#8a7f72'), shell: rgb('#857a6e') },
    rock: { face: rgb('#7d776f'), frame: rgb('#aaa49a'), coping: rgb('#b0aaa0'), wall: rgb('#77716a'), shell: rgb('#716b64') },
};
const DARK = rgb('#0a0a0b', VOID), LIGHT = rgb('#fff3d6', LAMP), TILES = rgb('#dcdad3', BORE), LINING = rgb('#8a877f', BORE);
const WALK = rgb('#a3a19b', BORE), GUTTER = rgb('#9b988f'), ROCK_FLOOR = rgb('#7d7262', BORE), OUT_FLOOR = rgb('#8a8378');

type V3 = [number, number, number];
interface Frame { at(s: number, z?: number): Station; into: V3; out: V3; left: V3 }

/** A straight frame from a world point along a unit way: s metres along, `shift` metres to the left. */
function frameOf(x: number, y: number, ux: number, uy: number, shift = 0): Frame {
    const unit = metresPerUnit(y), lx = uy, ly = -ux;
    return {
        at: (s, z = 0) => ({ x: x + (ux * s + lx * shift) / unit, y: y + (uy * s + ly * shift) / unit, z, ax: lx / unit, ay: ly / unit }),
        into: [ux, -uy, 0], out: [-ux, uy, 0], left: [lx, -ly, 0],
    };
}

/** An opening from its right foot over its top to its left foot; a round one springs at `spring`. */
function opening(w: number, h: number, round: boolean, spring = SPRING * h): Corner[] {
    if (!round) return [[-w, 0], [-w, h], [w, h], [w, 0]];
    const out: Corner[] = [[-w, 0]];
    for (let k = 0; k <= ARCH; k++) {
        const a = Math.PI * (1 - k / ARCH);
        out.push([w * Math.cos(a), spring + (h - spring) * Math.sin(a)]);
    }
    out.push([w, 0]);
    return out;
}

const scale = (v: V3, k: number): V3 => [v[0] * k, v[1] * k, v[2] * k];
const add = (...vs: V3[]): V3 => vs.reduce((a, v) => [a[0] + v[0], a[1] + v[1], a[2] + v[2]], [0, 0, 0] as V3);

/** A quad of four [station, corner] points in order, facing `dir`. */
function quad(b: MeshBuilder, pts: [Station, Corner][], colour: number, dir: V3): void {
    b.quad(pts.map(([s, c]) => b.at(s, c, colour)) as [number, number, number, number], dir);
}

/** The direction across-and-up of a cross-section edge from a to b, toward the axis line at height `mid` (sign -1) or away (1). */
function across(fr: Frame, a: Corner, b: Corner, mid: number, sign: number): V3 {
    const t = (a[0] + b[0]) / 2, u = (a[1] + b[1]) / 2 - mid, l = Math.hypot(t, u) || 1;
    return add(scale(fr.left, sign * t / l), [0, 0, sign * u / l]);
}

/**
 * A head wall at `so` (its frame ring; its front stands RECESS out of it): a wall from `bottom` to `top(t)` across from
 * -right to left with a hole of the opening and FRAME round it, a reveal, a lighter frame ring, its top with a coping.
 */
function headWall(b: MeshBuilder, fr: Frame, so: number, floor: number, w: number, h: number, round: boolean, top: (t: number) => number, left: number, right: number, look: Look, back = true): void {
    const sf = so - RECESS, sb = sf + FACE, F = fr.at(sf), B = fr.at(sb), O = fr.at(so), Fc = fr.at(sf - COPE), Bc = fr.at(sb + COPE);
    const hole = opening(w + FRAME, h + FRAME, round, SPRING * h).map(([t, u]): Corner => [t, floor + u]);
    const inner = opening(w, h, round).map(([t, u]): Corner => [t, floor + u]);
    const bottom = floor - SINK, edge = w + FRAME;
    // The columns: the right wing, the opening's corners over the top, the left wing.
    const cols: [number, number][] = [];
    const wing = (t0: number, t1: number) => {
        const ts = [t0];
        for (let t = Math.ceil(t0 + 1e-6); t < t1 - 1e-6; t++) ts.push(t);
        ts.push(t1);
        return ts;
    };
    for (const t of wing(-right, -edge)) cols.push([t, bottom]);
    for (const [t, u] of hole.slice(1, -1)) cols.push([t, u]);
    for (const t of wing(edge, left)) cols.push([t, bottom]);
    for (let k = 0; k + 1 < cols.length; k++) {
        const [[t0, u0], [t1, u1]] = [cols[k], cols[k + 1]];
        if (Math.abs(t1 - t0) < 1e-4) continue;
        const pts: Corner[] = [[t0, u0], [t1, u1], [t1, top(t1) - 0.3], [t0, top(t0) - 0.3]];
        quad(b, pts.map(c => [F, c]), look.face, fr.out);
        if (back) quad(b, pts.map(c => [B, c]), look.face, fr.into);
        // The coping: a band 0.3 m deep that stands COPE proud, and its top.
        quad(b, [[Fc, [t0, top(t0) - 0.3]], [Fc, [t1, top(t1) - 0.3]], [Fc, [t1, top(t1)]], [Fc, [t0, top(t0)]]], look.coping, fr.out);
        quad(b, [[Bc, [t0, top(t0) - 0.3]], [Bc, [t1, top(t1) - 0.3]], [Bc, [t1, top(t1)]], [Bc, [t0, top(t0)]]], look.coping, fr.into);
        quad(b, [[Fc, [t0, top(t0)]], [Fc, [t1, top(t1)]], [Bc, [t1, top(t1)]], [Bc, [t0, top(t0)]]], look.coping, [0, 0, 1]);
        quad(b, [[Fc, [t0, top(t0) - 0.3]], [Fc, [t1, top(t1) - 0.3]], [F, [t1, top(t1) - 0.3]], [F, [t0, top(t0) - 0.3]]], look.coping, [0, 0, -1]);
    }
    // The sill under the opening, the wall's ends.
    quad(b, [[F, [-edge, bottom]], [F, [edge, bottom]], [F, [edge, floor]], [F, [-edge, floor]]], look.face, fr.out);
    for (const [t, side] of [[left, 1], [-right, -1]] as const) {
        quad(b, [[Fc, [t, bottom]], [Bc, [t, bottom]], [Bc, [t, top(t)]], [Fc, [t, top(t)]]], look.coping, scale(fr.left, side));
    }
    // The reveal from the front into the frame, and the frame ring.
    const mid = floor + SPRING * h;
    for (let k = 0; k + 1 < hole.length; k++) {
        quad(b, [[F, hole[k]], [F, hole[k + 1]], [O, hole[k + 1]], [O, hole[k]]], look.face, across(fr, hole[k], hole[k + 1], mid, -1));
        quad(b, [[O, hole[k]], [O, hole[k + 1]], [O, inner[k + 1]], [O, inner[k]]], look.frame, fr.out);
    }
}

/** The outer shell of a tube from s0 to s1 (floors f0, f1): its walls from under the floor, its vault or roof. */
function shell(b: MeshBuilder, fr: Frame, s0: number, s1: number, f0: number, f1: number, w: number, h: number, round: boolean, colour: number): void {
    const corners = opening(w + T, h + T, round, SPRING * h);
    corners[0] = [-(w + T), -SINK];
    corners[corners.length - 1] = [w + T, -SINK];
    const A = { ...fr.at(s0), z: f0 }, Bs = { ...fr.at(s1), z: f1 };
    for (let k = 0; k + 1 < corners.length; k++) quad(b, [[A, corners[k]], [A, corners[k + 1]], [Bs, corners[k + 1]], [Bs, corners[k]]], colour, across(fr, corners[k], corners[k + 1], SPRING * h, 1));
}

/** Walls of a cutting along samples [s, left top, right top, floor], their inner faces `wt` from the axis; an end face at the first sample. */
function cuttingWalls(b: MeshBuilder, fr: Frame, rows: [number, number, number, number][], wt: number, look: Look, floorStrip: number): void {
    for (const side of [1, -1]) {
        const ti = side * wt, to = side * (wt + WALL), pick = (r: [number, number, number, number]): [number, number, number] => [r[0], side > 0 ? r[1] : r[2], r[3]];
        for (let k = 0; k + 1 < rows.length; k++) {
            const [[s0, h0, f0], [s1, h1, f1]] = [pick(rows[k]), pick(rows[k + 1])], A = fr.at(s0), B = fr.at(s1);
            quad(b, [[A, [ti, f0 - 1]], [B, [ti, f1 - 1]], [B, [ti, h1 - 0.3]], [A, [ti, h0 - 0.3]]], look.wall, scale(fr.left, -side));
            quad(b, [[A, [to, f0 - SINK]], [B, [to, f1 - SINK]], [B, [to, h1 - 0.3]], [A, [to, h0 - 0.3]]], look.wall, scale(fr.left, side));
            // The coping: proud on both faces, and its top.
            const ci = ti - side * COPE, co = to + side * COPE;
            quad(b, [[A, [ci, h0 - 0.3]], [B, [ci, h1 - 0.3]], [B, [ci, h1]], [A, [ci, h0]]], look.coping, scale(fr.left, -side));
            quad(b, [[A, [co, h0 - 0.3]], [B, [co, h1 - 0.3]], [B, [co, h1]], [A, [co, h0]]], look.coping, scale(fr.left, side));
            quad(b, [[A, [ci, h0]], [B, [ci, h1]], [B, [co, h1]], [A, [co, h0]]], look.coping, [0, 0, 1]);
            quad(b, [[A, [ci, h0 - 0.3]], [B, [ci, h1 - 0.3]], [B, [ti, h1 - 0.3]], [A, [ti, h0 - 0.3]]], look.coping, [0, 0, -1]);
            // The floor between the way's edge and the wall.
            if (floorStrip > 0) quad(b, [[A, [side * floorStrip, f0 + 0.04]], [B, [side * floorStrip, f1 + 0.04]], [B, [ti, f1 + 0.04]], [A, [ti, f0 + 0.04]]], GUTTER, [0, 0, 1]);
        }
        const [s0, h0, f0] = pick(rows[0]), A = fr.at(s0), away = rows.length > 1 && rows[1][0] > s0 ? fr.out : fr.into;
        quad(b, [[A, [ti - side * COPE, f0 - SINK]], [A, [to + side * COPE, f0 - SINK]], [A, [to + side * COPE, h0]], [A, [ti - side * COPE, h0]]], look.coping, away);
    }
}

/** The floor of an open stretch of a tunnel line from s0 to s1: the road's paint in the open, a floor to `w`. */
function openFloor(b: MeshBuilder, fr: Frame, line: Line, s0: number, s1: number, f0: number, f1: number, w: number): void {
    const A = { ...fr.at(s0), z: f0 }, B = { ...fr.at(s1), z: f1 }, h = line.h;
    if (line.deck! > 3 && line.code < RAIL_CODE) {
        const paint = paintOf(line, PAINT);
        b.quad([b.at(A, [-h, 0.03], paint, -h, h), b.at(A, [h, 0.03], paint, h, h), b.at(B, [h, 0.03], paint, h, h), b.at(B, [-h, 0.03], paint, -h, h)], [0, 0, 1]);
        for (const side of [1, -1]) quad(b, [[A, [side * h, 0.03]], [B, [side * h, 0.03]], [B, [side * w, 0.03]], [A, [side * w, 0.03]]], GUTTER, [0, 0, 1]);
    } else quad(b, [[A, [-w, 0.03]], [B, [-w, 0.03]], [B, [w, 0.03]], [A, [w, 0.03]]], OUT_FLOOR, [0, 0, 1]);
}

/** The station `s` metres along a line from vertex `from` toward vertex `to`, its height; past its end, its last. */
function floorAlong(line: Line, from: number, to: number, s: number): number {
    const step = Math.sign(to - from);
    let v = from, rest = s;
    for (;;) {
        const w = v + step, length = Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);
        if (rest <= length || w === to) return line.z[v] + (line.z[w] - line.z[v]) * Math.min(1, rest / (length || 1));
        rest -= length;
        v = w;
    }
}

/** The metres from an end of a line that its part takes from the bore: the bore draws past them. */
export function boreCut(line: Line, end: 0 | 1): number {
    const n = line.z.length, v = end ? n - 1 : 0, found = partAt(line.xy[2 * v], line.xy[2 * v + 1]), p = found && drawerOf(found);
    if (!p) return 0;
    if (p.kind === 'box' || (p.kind === 'skip' && p.rung.startsWith('underpass'))) return 1e9;
    if (p.kind === 'free') return 0.8 * (p.h + T) + 1;
    if (p.kind === 'arch' || p.kind === 'twin') return fitOf(p)!.open;
    return 0;
}

/** The part at an end of a line, built when its OSM end is this end. Returns whether a part stands there (or none is wanted). */
export function buildPart(b: MeshBuilder, line: Line, end: 0 | 1): boolean {
    const n = line.z.length, from = end ? n - 1 : 0, to = end ? 0 : n - 1, p = partAt(line.xy[2 * from], line.xy[2 * from + 1]);
    if (!p) return false;
    const look = LOOKS[p.look] ?? LOOKS.concrete, fl = (s: number) => floorAlong(line, from, to, Math.max(0, s));
    const fr = frameOf(p.x, p.y, p.ux, p.uy, p.shift);
    if (p.kind === 'arch' || p.kind === 'twin') {
        const f = fitOf(p)!, so = f.open, floor = fl(so), w = p.w, h = p.h, edge = w + T + FRAME;
        const top = (t: number) => {
            const k = Math.max(-16, Math.min(16, t)) + 16, i = Math.min(31, Math.floor(k)), row = f.tops[i] + (f.tops[i + 1] - f.tops[i]) * (k - i);
            return Math.min(f.head, Math.max(row, f.head - Math.max(0, Math.abs(t) - edge) * 1.5));
        };
        headWall(b, fr, so, floor, w, h, p.round, top, edge + f.wl, edge + f.wr, look);
        shell(b, fr, so - RECESS + FACE, so + Math.max(1, f.collar) + 1, fl(so - RECESS + FACE), fl(so + Math.max(1, f.collar) + 1), w, h, p.round, look.shell);
        if (so > 0.5) {
            // The cutting: walls back to the OSM end, the floor in the open.
            const wt = w + FRAME + 0.3, ss: number[] = [];
            for (let k = 0; k < so - RECESS - 0.2; k++) ss.push(k);
            ss.push(so - RECESS);
            const pickRow = (row: number[], s: number) => row[Math.min(row.length - 1, Math.round(s))];
            cuttingWalls(b, fr, ss.map(s => [s, pickRow(f.L, s), pickRow(f.R, s), fl(s)]), wt, look, 0);
            openFloor(b, fr, line, 0, so - RECESS, fl(0), fl(so - RECESS), w);
            for (const side of [1, -1]) quad(b, [[fr.at(0, fl(0)), [side * w, 0.04]], [fr.at(so - RECESS, fl(so - RECESS)), [side * w, 0.04]], [fr.at(so - RECESS, fl(so - RECESS)), [side * wt, 0.04]], [fr.at(0, fl(0)), [side * wt, 0.04]]], GUTTER, [0, 0, 1]);
        }
        voidFan(b, fr, so + 0.7, floor, w, h, p.round);
        return true;
    }
    if (p.kind === 'free') {
        freeEnd(b, fr, line, p, fl, look);
        return true;
    }
    if (p.kind === 'box') {
        underpass(b, line, p, look);
        return true;
    }
    return p.kind === 'skip' || p.kind === 'none' ? p.kind === 'skip' : false;
}

/** The dark void that stands in an opening where the terrain draws coarse (VOID_LOD of structures.wgsl). */
function voidFan(b: MeshBuilder, fr: Frame, s: number, floor: number, w: number, h: number, round: boolean): void {
    const st = fr.at(s), rim = opening(w, h, round).map(([t, u]) => b.at(st, [t, floor + u], DARK)), centre = b.at(st, [0, floor + h / 2], DARK);
    rim.forEach((v, k) => b.triangle(centre, v, rim[(k + 1) % rim.length], fr.out));
}

/** The free tube end: the tube's own shell, cut on a slant that leans back over the opening. */
function freeEnd(b: MeshBuilder, fr: Frame, line: Line, p: Placement, fl: (s: number) => number, look: Look): void {
    const w = p.w, h = p.h, len = p.free!.len, slant = 0.8 * (h + T), cut = slant + 1;
    const inner = opening(w, h, p.round), outer = opening(w + T, h + T, p.round, SPRING * h);
    const start = (u: number) => Math.max(0, u) / (h + T) * slant;
    const at = (s: number, c: Corner) => [{ ...fr.at(s), z: fl(s) }, c] as [Station, Corner];
    for (let k = 0; k + 1 < inner.length; k++) {
        const [a, c] = [inner[k], inner[k + 1]], [A, C] = [outer[k], outer[k + 1]];
        // Inside up to `cut` (the bore draws past it), outside to the shell's end, and the rim on the slant.
        quad(b, [at(start(a[1]), a), at(start(c[1]), c), at(cut, c), at(cut, a)], k === 0 || k === inner.length - 2 ? TILES : LINING, across(fr, a, c, SPRING * h, -1));
        quad(b, [at(start(A[1]), A), at(start(C[1]), C), at(len, C), at(len, A)], look.shell, across(fr, A, C, SPRING * h, 1));
        quad(b, [at(start(a[1]), a), at(start(c[1]), c), at(start(C[1]), C), at(start(A[1]), A)], look.coping, add(fr.out, [0, 0, 0.8]));
    }
    // The shell's walls down into the ground, and the floor in the open part.
    for (const side of [1, -1]) quad(b, [at(0, [side * (w + T), -SINK]), at(len, [side * (w + T), -SINK]), at(len, [side * (w + T), 0]), at(0, [side * (w + T), 0])], look.shell, scale(fr.left, side));
    quad(b, [at(0, [-(w + T), -SINK]), at(0, [w + T, -SINK]), at(0, [w + T, 0]), at(0, [-(w + T), 0])], look.coping, fr.out);
    openFloor(b, fr, line, 0, cut, fl(0), fl(cut), w);
    for (const side of [1, -1]) quad(b, [at(0, [side * w, 0]), at(0, [side * (w + T), 0]), at(cut, [side * (w + T), 0]), at(cut, [side * w, 0])], look.coping, [0, 0, 1]);
    voidFan(b, fr, cut, fl(cut), w, h, p.round);
}

/** An underpass as one part from its owner end A to its other end B: the box, its lamps, two head walls and their wing walls. */
function underpass(b: MeshBuilder, line: Line, p: Placement, look: Look): void {
    const [A, B] = p.ends!, unit = metresPerUnit(A.y), dx = (B.x - A.x) * unit, dy = (B.y - A.y) * unit, length = Math.hypot(dx, dy) || 1;
    const fr = frameOf(A.x, A.y, dx / length, dy / length), w = p.w, hb = p.hb!, h = line.h, road = line.deck! > 3 && line.code < RAIL_CODE;
    const fl = (s: number) => A.floor + (B.floor - A.floor) * Math.max(0, Math.min(1, s / length));
    const st = (s: number) => ({ ...fr.at(s), z: fl(s) });
    const S = st(0), E = st(length);
    // Inside: floor, walls, ceiling, lamps.
    if (road) {
        const paint = paintOf(line, PAINT | BORE);
        b.quad([b.at(S, [-h, 0.03], paint, -h, h), b.at(S, [h, 0.03], paint, h, h), b.at(E, [h, 0.03], paint, h, h), b.at(E, [-h, 0.03], paint, -h, h)], [0, 0, 1]);
        for (const side of [1, -1]) quad(b, [[S, [side * h, 0.03]], [E, [side * h, 0.03]], [E, [side * w, 0.15]], [S, [side * w, 0.15]]], WALK, [0, 0, 1]);
    } else quad(b, [[S, [-w, 0.03]], [E, [-w, 0.03]], [E, [w, 0.03]], [S, [w, 0.03]]], line.code >= RAIL_CODE ? ROCK_FLOOR : WALK, [0, 0, 1]);
    for (const side of [1, -1]) quad(b, [[S, [side * w, 0]], [E, [side * w, 0]], [E, [side * w, hb]], [S, [side * w, hb]]], TILES, scale(fr.left, -side));
    quad(b, [[S, [-w, hb]], [E, [-w, hb]], [E, [w, hb]], [S, [w, hb]]], LINING, [0, 0, -1]);
    posts(S, E, length, (q, r) => b.quad([b.at(q, [-0.15, hb - 0.05], LIGHT), b.at(q, [0.15, hb - 0.05], LIGHT), b.at(r, [0.15, hb - 0.05], LIGHT), b.at(r, [-0.15, hb - 0.05], LIGHT)], [0, 0, -1]), 1.2, 6);
    // Outside: walls, slab.
    for (const side of [1, -1]) quad(b, [[S, [side * (w + T), -SINK]], [E, [side * (w + T), -SINK]], [E, [side * (w + T), hb + SLAB]], [S, [side * (w + T), hb + SLAB]]], look.shell, scale(fr.left, side));
    quad(b, [[S, [-(w + T), hb + SLAB]], [E, [-(w + T), hb + SLAB]], [E, [w + T, hb + SLAB]], [S, [w + T, hb + SLAB]]], look.shell, [0, 0, 1]);
    // Each end: a head wall that spans to its wing walls' outer faces, and the wing walls out along the approach.
    const wt = w + FRAME + 0.3;
    for (const e of [A, B] as BoxEnd[]) {
        const away = e === A ? 1 : -1, ef = frameOf(e.x, e.y, (away * dx) / length, (away * dy) / length);
        headWall(b, ef, 0, e.floor, w, hb, false, () => e.head, wt + WALL, wt + WALL, look, false);
        if (e.len >= 1) {
            const rows: [number, number, number, number][] = [];
            for (let k = Math.round(e.len); k >= 0; k--) rows.push([-k - (k === 0 ? RECESS : 0), e.L[k], e.R[k], e.F[k]]);
            cuttingWalls(b, ef, rows, wt, look, h);
        }
    }
}

/** Words of a box (BOX of node.ts): its start in world units, its unit way, depth, half-width, roof, floor, rise. */
function box(fr: Frame, s: number, depth: number, half: number, roof: number, floor: number, rise: number, ux: number, uy: number): number[] {
    const st = fr.at(s);
    return [st.x, st.y, ux, uy, depth, half, roof, floor, rise];
}

/** The boxes of a placement in which the terrain and the ground plan draw nothing; the end that draws the part holds them. */
export function partBoxes(p: Placement): number[] {
    const fr = frameOf(p.x, p.y, p.ux, p.uy, p.shift), out: number[] = [];
    if (p.kind === 'arch' || p.kind === 'twin') {
        const f = fitOf(p)!, so = f.open, sf = so - RECESS, floor = f.floor;
        out.push(...box(fr, sf, Math.max(12, FACE + f.collar + 2), p.w + FRAME, floor + p.h + T + 0.1, floor + 0.03, f.grade ?? 0, p.ux, p.uy));
        if (so > 0.5) {
            const top = Math.max(...f.L, ...f.R) + 5, f0 = f.F[0];
            out.push(...box(fr, 0, sf, p.w + FRAME + 0.3 + 0.25, top, f0 + 0.25, (floor - f0) / Math.max(sf, 0.1), p.ux, p.uy));
        }
    } else if (p.kind === 'free') {
        out.push(...box(fr, 0, p.free!.len + 2, p.w + T, p.floor0 + p.h + T + 0.1, p.floor0 + 0.03, 0, p.ux, p.uy));
    } else if (p.kind === 'box') {
        const [A, B] = p.ends!, unit = metresPerUnit(A.y), dx = (B.x - A.x) * unit, dy = (B.y - A.y) * unit, length = Math.hypot(dx, dy) || 1;
        const ux = dx / length, uy = dy / length, af = frameOf(A.x, A.y, ux, uy);
        out.push(...box(af, -RECESS, length + 2 * RECESS, p.w + T, A.floor + p.hb! + SLAB - 0.05, A.floor + 0.03, (B.floor - A.floor) / length, ux, uy));
        for (const e of [A, B]) {
            if (e.len < 1) continue;
            const away = e === A ? 1 : -1, ef = frameOf(e.x, e.y, away * ux, away * uy), n = Math.round(e.len), fo = e.F[n];
            out.push(...box(ef, -n, n - RECESS, p.w + FRAME + 0.3 + 0.25, e.head + 5, fo + 0.25, (e.F[0] - fo) / n, away * ux, away * uy));
        }
    }
    return out;
}
