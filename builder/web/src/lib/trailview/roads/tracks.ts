// PROTOTYPE road system (a study; never merges). The 3D detail of a corridor near the eye: a track's
// sleepers, rails, rack and funicular cable on its ballast bed, and the treads and risers of steps.
// Their vertices keep their place to a fraction of a world unit (`fine`), as a world unit is about
// 2.5 cm and a rail's head 6 cm wide; corridor.wgsl sinks them past DETAIL_M, where the paint shows.
import { metresPerUnit, PATH, RAIL_CODE, type Line } from '../terrain/bins';
import type { MeshBuilder } from '../structures/mesh';
import { BED_M, classOf, LIFT_M, LOOK } from './section';
import type { Station } from './stations';

/** The colour word of a corridor vertex, as corridorColour of corridor.ts. */
const colourOf = (mat: number, code: number, prio: number) => (mat | (code << 8) | ((prio << 4) << 16) | (12 << 24)) >>> 0;

/** A rail's cross-section: metres across from its middle and up from the sleeper's top, for a rail 0.125 m high. */
const RAIL: [number, number][] = [[-0.05, 0], [-0.05, 0.012], [-0.009, 0.028], [-0.009, 0.088], [-0.03, 0.097], [-0.031, 0.125], [0.031, 0.125], [0.03, 0.097], [0.009, 0.088], [0.009, 0.028], [0.05, 0.012], [0.05, 0]];
/** The edge of RAIL that is the rail's running head. */
const HEAD_EDGE = 5;
/** A sleeper: its width along the track and its height over the ballast's top, which hides its lower part. */
const SLEEPER_W = 0.26, SLEEPER_UP = 0.05;
/** A riser of steps, and the least and most tread. */
const RISE_M = 0.16, TREAD_MIN_M = 0.26, TREAD_MAX_M = 1.6;

interface Gauge { rail: number; sleeper: number; scale: number; rack: boolean; cable: boolean; wood: boolean }

/** The track's look: the middle of each rail from the track's middle, a sleeper's half-length, the rail's size, a rack, a funicular's cable. */
function gaugeOf(line: Line): Gauge {
    const narrow = ((line.code - RAIL_CODE) & 1) !== 0, cls = classOf(line);
    return {
        rail: narrow ? 0.5 + 0.03 : 0.7175 + 0.035, sleeper: narrow ? 0.9 : 1.3, scale: narrow ? 1 : 1.2,
        rack: line.code >= RAIL_CODE + 2 || cls === 'rack', cable: cls === 'funicular', wood: cls === 'funicular' || cls === 'rack',
    };
}

/** A vertex at plan point (x, y) in world units, kept to 1/128 of a unit: the fraction rides in the half-width word (corridor_fine of looks.wgsl). */
function fine(b: MeshBuilder, x: number, y: number, z: number, colour: number, across: number, along: number): number {
    const rx = Math.round(x), ry = Math.round(y);
    const q = Math.round((x - rx + 0.5) * 127) * 256 + Math.round((y - ry + 0.5) * 255);
    return b.vertex(rx, ry, z, colour, across, (q / 32767) * 16, along);
}

/** The station at `along` metres between stations (binary search), with its height. */
function stationAt(st: Station[], along: number): Station {
    let lo = 0, hi = st.length - 1;
    while (hi - lo > 1) {
        const m = (lo + hi) >> 1;
        if (st[m].along <= along) lo = m;
        else hi = m;
    }
    const s = st[lo], t = st[hi], u = Math.max(0, Math.min(1, (along - s.along) / Math.max(t.along - s.along, 1e-6)));
    const mix = (p: number, q: number) => p + (q - p) * u;
    const sl = s.sleepers !== undefined && t.sleepers !== undefined ? mix(s.sleepers, t.sleepers) : undefined;
    return { ...s, x: mix(s.x, t.x), y: mix(s.y, t.y), z: mix(s.z, t.z), ax: mix(s.ax, t.ax), ay: mix(s.ay, t.ay), along, sleepers: sl, j: undefined };
}

/** The unit vector along a station pair in world units per metre. */
function ahead(s: Station): [number, number] {
    // The left vector turned a quarter clockwise in plan (y runs south).
    return [-s.ay, s.ax];
}

/** The detail of the segments of a line that a node owns: owned(j) for the segment from station j. */
export function tracks(b: MeshBuilder, line: Line, st: Station[], owned: (j: number) => boolean, prio: number): void {
    if (line.code >= RAIL_CODE && line.sleepers) track(b, line, st, owned, prio);
    else if (line.kind === PATH && classOf(line) === 'steps') steps(b, line, st, owned, prio);
}

function track(b: MeshBuilder, line: Line, st: Station[], owned: (j: number) => boolean, prio: number): void {
    const g = gaugeOf(line), code = line.code, top = LIFT_M + BED_M + SLEEPER_UP;
    const sleeperColour = colourOf(g.wood ? LOOK.SLEEPER_WOOD : LOOK.SLEEPER, code, prio);
    // The sleepers: boxes across the track at each whole count.
    for (let j = 0; j + 1 < st.length; j++) {
        if (!owned(j)) continue;
        const s = st[j], t = st[j + 1];
        if (s.sleepers === undefined || t.sleepers === undefined || t.sleepers === s.sleepers) continue;
        const lo = Math.min(s.sleepers, t.sleepers), hi = Math.max(s.sleepers, t.sleepers);
        for (let k = Math.ceil(lo); k < hi; k++) {
            const along = s.along + ((k - s.sleepers) / (t.sleepers - s.sleepers)) * (t.along - s.along);
            box(b, stationAt(st, along), SLEEPER_W, g.sleeper, top - 0.12, top, sleeperColour, k);
        }
    }
    // The rails: the profile swept along the stations, the head in its own material.
    const steel = colourOf(LOOK.RAIL, code, prio), head = colourOf(LOOK.RAIL_HEAD, code, prio);
    for (const side of [1, -1]) sweep(b, st, owned, RAIL.map(([a, up]) => [side * g.rail + a, top + up * g.scale]), k => (k === HEAD_EDGE ? head : steel));
    if (g.rack) {
        // A ladder rack: two plates 0.12 m apart with their rungs between, which the rack's look draws on its top.
        const rack = colourOf(LOOK.RACK, code, prio);
        sweep(b, st, owned, [[-0.08, top], [-0.08, top + 0.09], [0.08, top + 0.09], [0.08, top]], () => rack);
    }
    if (g.cable) {
        // A funicular's haul cable on its rollers: rollers every 8 m, the cable a band 3 cm wide over them.
        const cable = colourOf(LOOK.CABLE, code, prio);
        sweep(b, st, owned, [[-0.015, top + 0.06], [-0.015, top + 0.09], [0.015, top + 0.09], [0.015, top + 0.06]], () => cable);
        for (let j = 0; j + 1 < st.length; j++) {
            if (!owned(j)) continue;
            for (let a = Math.ceil(st[j].along / 8) * 8; a < st[j + 1].along; a += 8) box(b, stationAt(st, a), 0.1, 0.16, top - 0.05, top + 0.06, colourOf(LOOK.RAIL, code, prio), stationAt(st, a).sleepers ?? 0);
        }
    }
}

/** A box `len` metres along and 2 × `half` across at station s, from `z0` to `z1` over the station's height. */
function box(b: MeshBuilder, s: Station, len: number, half: number, z0: number, z1: number, colour: number, along: number): void {
    const [fx, fy] = ahead(s), v: number[] = [];
    for (const [da, dl, dz] of [[1, -1, z0], [1, 1, z0], [-1, 1, z0], [-1, -1, z0], [1, -1, z1], [1, 1, z1], [-1, 1, z1], [-1, -1, z1]]) {
        const a = da * half, l = (dl * len) / 2;
        v.push(fine(b, s.x + s.ax * a + fx * l, s.y + s.ay * a + fy * l, s.z + dz, colour, a, along));
    }
    const m = metresPerUnit(s.y), up: [number, number, number] = [0, 0, 1];
    const left: [number, number, number] = [s.ax * m, -s.ay * m, 0], fwd: [number, number, number] = [fx * m, -fy * m, 0];
    const neg = (o: [number, number, number]): [number, number, number] => [-o[0], -o[1], -o[2]];
    b.quad([v[4], v[5], v[6], v[7]], up);
    b.quad([v[0], v[1], v[5], v[4]], left);
    b.quad([v[3], v[2], v[6], v[7]], neg(left));
    b.quad([v[1], v[2], v[6], v[5]], fwd);
    b.quad([v[0], v[3], v[7], v[4]], neg(fwd));
}

/** A cross-section (metres across, up) swept along the owned segments; the face of edge k takes colour(k). */
function sweep(b: MeshBuilder, st: Station[], owned: (j: number) => boolean, shape: number[][], colour: (k: number) => number): void {
    let area = 0;
    for (let k = 0; k < shape.length; k++) area += shape[k][0] * shape[(k + 1) % shape.length][1] - shape[(k + 1) % shape.length][0] * shape[k][1];
    const made = new Map<string, number>();
    // A vertex of point k at station j in colour c, shared by the faces of that colour.
    const at = (j: number, k: number, c: number) => {
        const key = `${j}|${k}|${c}`, s = st[j], [a, up] = shape[k];
        let i = made.get(key);
        if (i === undefined) made.set(key, (i = fine(b, s.x + s.ax * a, s.y + s.ay * a, s.z + up, c, a, s.sleepers ?? s.along)));
        return i;
    };
    for (let j = 0; j + 1 < st.length; j++) {
        if (!owned(j)) continue;
        for (let k = 0; k + 1 < shape.length; k++) {
            const dx = shape[k + 1][0] - shape[k][0], dy = shape[k + 1][1] - shape[k][1], c = colour(k);
            b.quad([at(j, k, c), at(j, k + 1, c), at(j + 1, k + 1, c), at(j + 1, k, c)], b.outward(st[j], st[j + 1], area > 0 ? [dy, -dx] : [-dy, dx]));
        }
    }
}

/**
 * Steps along a line: treads about RISE_M apart in height, each level at the higher end of its stretch, so
 * the flight stands on the sloped core; risers face down the flight; cheeks close the sides down to the core.
 */
function steps(b: MeshBuilder, line: Line, st: Station[], owned: (j: number) => boolean, prio: number): void {
    const total = st[st.length - 1].along, rise = Math.abs(st[st.length - 1].z - st[0].z);
    if (total < 0.5) return;
    const count = Math.max(1, Math.round(rise / RISE_M)), tread = Math.min(TREAD_MAX_M, Math.max(TREAD_MIN_M, total / count));
    const n = Math.max(1, Math.round(total / tread)), half = Math.max(0.6, Math.min(line.h * 0.8, 1.4));
    const treadColour = colourOf(LOOK.TREAD, line.code, prio), riser = colourOf(LOOK.RISER, line.code, prio);
    const lift = LIFT_M + 0.01;
    let prev = -Infinity, j = 0;
    for (let k = 0; k < n; k++) {
        const a0 = (k * total) / n, a1 = ((k + 1) * total) / n, s = stationAt(st, a0), t = stationAt(st, a1);
        while (j + 2 < st.length && st[j + 1].along <= a0) j++;
        const level = Math.max(s.z, t.z) + lift;
        if (!owned(j)) {
            prev = level;
            continue;
        }
        const at = (p: Station, a: number, z: number, c: number, along = p.along) => fine(b, p.x + p.ax * a, p.y + p.ay * a, z, c, a, along);
        // The tread: its `along` is metres back from its nosing, at its start going up the line, else at its end.
        const up = s.z <= t.z, ns = up ? 0 : a1 - a0, nt = up ? a1 - a0 : 0;
        b.quad([at(s, half, level, treadColour, ns), at(s, -half, level, treadColour, ns), at(t, -half, level, treadColour, nt), at(t, half, level, treadColour, nt)], [0, 0, 1]);
        const [fx, fy] = ahead(s), m = metresPerUnit(s.y), fwd: [number, number, number] = [fx * m, -fy * m, 0];
        if (k > 0 && Math.abs(level - prev) > 0.01) {
            const lo = Math.min(level, prev), hi = Math.max(level, prev), down: [number, number, number] = level > prev ? [-fwd[0], -fwd[1], 0] : fwd;
            b.quad([at(s, half, lo, riser), at(s, -half, lo, riser), at(s, -half, hi, riser), at(s, half, hi, riser)], down);
        }
        // Cheeks: from the tread down to under the core on each side.
        for (const side of [1, -1]) {
            const a = side * half, out: [number, number, number] = [s.ax * m * side, -s.ay * m * side, 0];
            b.quad([at(s, a, s.z - 0.05, riser), at(t, a, t.z - 0.05, riser), at(t, a, level, riser), at(s, a, level, riser)], out);
        }
        prev = level;
    }
}

