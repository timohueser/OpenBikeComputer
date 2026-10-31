// The faces of what stands beside a road: W-beam guardrails on posts, masonry retaining walls and delineators.
// A rail or a wall is a cross-section swept like the road's own (sweep.ts), and its heights come from standing.ts, so
// nothing here reads the ground in the vertex stage but a backfill's outer rows. Every face casts but the backfill's
// last, under the ground; the wall alone, seen from the sun, would let the sun past its foot.
import { LIFT_M } from '../terrain/bins';
import type { GroundAt } from './ground';
import { lookWord, type PieceBuilder } from './piece';
import { CF, MAT, type Part } from './section';
import { allowed, coneOf, edgeOf, WALL_OUT_M, type Standing } from './standing';
import type { Station } from './stations';
import { countOf, point, quad, type Run } from './sweep';

/** A point of a section: metres past the edge, metres over the edge, its material, whether the far range keeps it. */
type Point = [number, number, number, boolean];

const { RAIL, RAIL_POST: POST, RAIL_BLOCK: BLOCK, DELI, DELI_BAND: BAND, REFLECTOR, WALL, CAP, GROUND } = MAT;
/** The W-beam, closed. Its middle fold faces the road. */
const BEAM: Point[] = [[0.42, 0.44, RAIL, true], [0.36, 0.49, RAIL, false], [0.4, 0.595, RAIL, true], [0.36, 0.7, RAIL, false], [0.42, 0.75, RAIL, true], [0.45, 0.595, RAIL, true], [0.42, 0.44, RAIL, true]];
/** A rail that does not stand has sunk this far: its beam's top lies under the deepest fill at its foot. */
const RAIL_SINK_M = 1.9;
/** A post and the blockout that holds the beam off it, one L: metres past the edge, metres over the edge, the material of the face to the next point. */
const L: [number, number, number][] = [[0.57, 0, POST], [0.67, 0, POST], [0.67, 0.71, POST], [0.44, 0.71, BLOCK], [0.44, 0.5, BLOCK], [0.57, 0.5, POST]];
/** A post's half width along, the metres between posts, and how far its foot lies under the edge: under the fill's cone at the post, on any fill. */
const POST_HALF_M = 0.04, POST_M = 2, POST_FOOT_M = 1.5;
/** A wall that does not stand has sunk this far under the edge. */
const WALL_SINK_M = 0.6;
/** Its face past the edge, its capstone's front and back, how far its foot lies under the edge. */
const FACE_M = 0.03, CAP_FRONT_M = -0.01, CAP_BACK_M = 0.37, FOOT_M = 0.25;
/** The capstone's underside under the wall's top, and its top over it. */
const CAP_UNDER_M = 0.08, CAP_OVER_M = 0.04;
/** The backfill leaves the capstone this far under the wall's top, and dives under the ground this far past WALL_OUT_M. */
const FILL_UNDER_M = 0.03, FILL_DIVE_M = 0.3;
/** A delineator: its middle past the edge, half its depth and width, its foot under the ground, then its band and top over the ground. */
const DELI_M = 0.55, DELI_HALF_M = [0.05, 0.06], DELI_Z = [-0.3, 0.74, 0.9, 1];
/** Metres between delineators on a straight and in a bend, and how far from a mouth, a town or a sidewalk they keep. */
const DELI_STEP_M = 40, DELI_BEND_M = 22, DELI_CLEAR_M = 5;
/** A way bends where it turns this far within BEND_STATIONS each way. */
const BEND_RAD = 0.35, BEND_STATIONS = 6;
/** A reflector's half width, its inset from the band's ends, and how far proud of the band it lies. */
const REFLECTOR_HALF_M = 0.03, REFLECTOR_INSET_M = 0.03, REFLECTOR_OUT_M = 0.004;

/** A cross-section's parts at station s of a side, from points past the edge. */
const section = (s: Station, side: 1 | -1, edge: number, points: Point[]): Part[] => points.map(([a, over, mat, far]) => ({ a: edge + a, dz: LIFT_M + over - side * s.bank * a, mat, flags: 0, far }));

/** The section of a wall `high` metres over the edge at station s, standing by `share`, its backfill's rows last. */
function wallSection(s: Station, side: 1 | -1, edge: number, share: number, high: number): Part[] {
    const top = share * high - (1 - share) * WALL_SINK_M, under = top - CAP_UNDER_M, over = top + CAP_OVER_M, fill = top - FILL_UNDER_M;
    const parts = section(s, side, edge, [
        [FACE_M, Math.min(-FOOT_M, top - 0.3), WALL, true], [FACE_M, under, WALL, true], [CAP_FRONT_M, under, CAP, false], [CAP_FRONT_M, over, CAP, false], [CAP_BACK_M, over, CAP, true], [CAP_BACK_M, fill, CAP, false],
        [WALL_OUT_M, fill, GROUND, true],
    ]);
    parts[parts.length - 1].flags = CF.FILL;
    parts.push({ a: edge + WALL_OUT_M + FILL_DIVE_M, dz: 0, mat: GROUND, flags: CF.MESH, far: true });
    return parts;
}

/** Faces between the sections of stations j and k, up to where a bend's fold stops them. A far face that casts goes to the casters. */
function strip(b: PieceBuilder, run: Run, side: 1 | -1, j: number, k: number, parts: Part[], others: Part[], far: boolean): void {
    const most = side > 0 ? 0 : 1, reachS = run.reach[2 * j + most], reachT = run.reach[2 * k + most];
    const at = (station: number, part: Part) => point(b, run, station, part, side * part.a, run.stations[station].h);
    let ps = at(j, parts[0]), pt = at(k, others[0]);
    for (let c = 1; c < parts.length && parts[c].a <= reachS && others[c].a <= reachT; c++) {
        if (far && !parts[c].far) continue;
        const qs = at(j, parts[c]), qt = at(k, others[c]);
        quad(!far ? b.near : parts[c].flags === CF.MESH ? b.far : b.casters, ps, qs, qt, pt, side > 0);
        ps = qs;
        pt = qt;
    }
}

/** A face of four vertices, counter-clockwise from outside. Its material is its first vertex's. */
const face = (list: number[], a: number, b: number, c: number, d: number) => list.push(a, b, c, a, c, d);

/** The vertices of a side about a point of the line, in metres past the edge, along the side's way and over the edge. */
function placer(b: PieceBuilder, run: Run, side: 1 | -1, s: Station, edge: number) {
    const base = s.z + side * s.bank * edge + run.lift + LIFT_M;
    return (a: number, al: number, over: number, mat: number) => {
        const out = side * (edge + a), on = side * al;
        return b.vertex(s.x + s.ax * out - s.ay * on, s.y + s.ay * out + s.ax * on, base + over, lookWord(mat, run.line.code, 0, run.prio, s.h), out, countOf(run, s));
    };
}

/** A post at station s, sunk by `sink`: near the eye its L with its ends; far off, its two sides and ends alone, on the same vertices. */
function post(b: PieceBuilder, run: Run, side: 1 | -1, s: Station, edge: number, sink: number): void {
    const place = placer(b, run, side, s, edge), foot = -POST_FOOT_M - sink;
    const lo = L.map(([a, over, mat]) => place(a, -POST_HALF_M, over ? over - sink : foot, mat)), hi = L.map(([a, over, mat]) => place(a, POST_HALF_M, over ? over - sink : foot, mat));
    for (let k = 1; k < L.length; k++) face(b.near, lo[k], lo[(k + 1) % L.length], hi[(k + 1) % L.length], hi[k]);
    // The L's ends, two quads about its inner corner.
    face(b.near, hi[5], hi[0], hi[1], hi[2]);
    face(b.near, hi[5], hi[2], hi[3], hi[4]);
    face(b.near, lo[0], lo[5], lo[2], lo[1]);
    face(b.near, lo[2], lo[5], lo[4], lo[3]);
    face(b.casters, lo[1], lo[2], hi[2], hi[1]);
    face(b.casters, lo[5], lo[0], hi[0], hi[5]);
    face(b.casters, hi[5], hi[0], hi[1], hi[2]);
    face(b.casters, lo[0], lo[5], lo[2], lo[1]);
}

/** A delineator at station s, with its foot's ground `ground` metres over the edge: white, with a black band and an amber reflector toward the road. */
function delineator(b: PieceBuilder, run: Run, side: 1 | -1, s: Station, edge: number, ground: number): void {
    const place = placer(b, run, side, s, edge), [depth, width] = DELI_HALF_M, near = DELI_M - depth, out = DELI_M + depth, z = DELI_Z.map(v => v + ground);
    const ring = (over: number, mat: number) => [place(near, -width, over, mat), place(near, width, over, mat), place(out, width, over, mat), place(out, -width, over, mat)];
    const rings = [ring(z[0], DELI), ring(z[1], BAND), ring(z[2], DELI), ring(z[3], DELI)];
    const sides = (list: number[], low: number[], high: number[]) => {
        for (let k = 0; k < 4; k++) face(list, low[k], low[(k + 1) % 4], high[(k + 1) % 4], high[k]);
    };
    sides(b.near, rings[0], rings[1]);
    sides(b.near, rings[1], rings[2]);
    sides(b.near, rings[2], rings[3]);
    face(b.near, ...rings[3] as [number, number, number, number]);
    sides(b.casters, rings[0], rings[3]);
    const shine = (al: number, over: number) => place(near - REFLECTOR_OUT_M, al, over, REFLECTOR);
    face(b.near, shine(-REFLECTOR_HALF_M, z[1] + REFLECTOR_INSET_M), shine(REFLECTOR_HALF_M, z[1] + REFLECTOR_INSET_M), shine(REFLECTOR_HALF_M, z[2] - REFLECTOR_INSET_M), shine(-REFLECTOR_HALF_M, z[2] - REFLECTOR_INSET_M));
}

/** The station a share u of the way from s to t. */
function between(s: Station, t: Station, u: number): Station {
    const f = (p: number, q: number) => p + (q - p) * u;
    return { ...s, x: f(s.x, t.x), y: f(s.y, t.y), z: f(s.z, t.z), ax: f(s.ax, t.ax), ay: f(s.ay, t.ay), along: f(s.along, t.along), bank: f(s.bank, t.bank) };
}

/** Whether a line turns more than BEND_RAD about station j. */
function bent(stations: Station[], j: number): boolean {
    const p = stations[Math.max(0, j - BEND_STATIONS)], q = stations[Math.min(stations.length - 1, j + BEND_STATIONS)];
    return p.ax * q.ax + p.ay * q.ay < Math.cos(BEND_RAD) * Math.hypot(p.ax, p.ay) * Math.hypot(q.ax, q.ay);
}

/** The stations of a far run: those of the far range where a detail stands, its ends, and its ramps, whose shape a chord would lose. */
function farOf(far: number[], share: Float64Array): number[] {
    const kept = new Uint8Array(share.length);
    for (const j of far) kept[j] = 1;
    return Array.from(share.keys()).filter(j => kept[j] || (share[j] < 1 && (share[j] > 0 || share[j - 1] > 0 || share[j + 1] > 0)));
}

/** One side of a line's details in the stations that `pieceAt` holds. */
export function roadside(run: Run & { far: number[] }, side: 1 | -1, standing: Standing, ground: GroundAt, owns: (s: Station) => boolean, pieceAt: (s: Station) => PieceBuilder): void {
    const { line, stations, far } = run, { rail, wall, high, free } = standing, last = stations.length - 1, delineated = allowed(line).railed;
    const edgeAt = (j: number) => edgeOf(line, stations[j]);
    const wallAt = (j: number) => wallSection(stations[j], side, edgeAt(j), wall[j], high[j]);
    const railAt = (j: number) => section(stations[j], side, edgeAt(j), BEAM.map(([a, over, mat, kept]) => [a, over - (1 - rail[j]) * RAIL_SINK_M, mat, kept]));
    for (let j = 0; j < last; j++) {
        const s = stations[j], t = stations[j + 1];
        if (!owns(s)) continue;
        const b = pieceAt(s);
        if (wall[j] > 0 || wall[j + 1] > 0) strip(b, run, side, j, j + 1, wallAt(j), wallAt(j + 1), false);
        if (rail[j] > 0 || rail[j + 1] > 0) {
            strip(b, run, side, j, j + 1, railAt(j), railAt(j + 1), false);
            for (let m = Math.ceil(s.along / POST_M) * POST_M; m < t.along; m += POST_M) {
                const u = (m - s.along) / (t.along - s.along), share = rail[j] + (rail[j + 1] - rail[j]) * u;
                if (share > 0) post(b, run, side, between(s, t, u), edgeAt(j), (1 - share) * RAIL_SINK_M);
            }
        }
        if (delineated && free[j] >= DELI_CLEAR_M && free[j + 1] >= DELI_CLEAR_M && !rail[j] && !rail[j + 1] && !wall[j] && !wall[j + 1]) {
            const step = bent(stations, j) ? DELI_BEND_M : DELI_STEP_M;
            for (let m = Math.ceil(s.along / step) * step; m < t.along; m += step) {
                const at = between(s, t, (m - s.along) / (t.along - s.along)), edge = edgeAt(j), reach = coneOf(line, DELI_M);
                const over = ground(at.x + side * at.ax * (edge + DELI_M), at.y + side * at.ay * (edge + DELI_M)) - at.z - side * at.bank * edge;
                if (!Number.isNaN(over)) delineator(b, run, side, at, edge, Math.max(-reach, Math.min(reach, over)));
            }
        }
    }
    for (const [share, sectionAt] of [[wall, wallAt], [rail, railAt]] as const) {
        const kept = farOf(far, share);
        for (let k = 0; k + 1 < kept.length; k++) {
            const j = kept[k], next = kept[k + 1], s = stations[j];
            if (owns(s) && share.subarray(j, next + 1).some(v => v > 0)) strip(pieceAt(s), run, side, j, next, sectionAt(j), sectionAt(next), true);
        }
    }
}
