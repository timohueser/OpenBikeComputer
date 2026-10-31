// What stands on a way near the eye: a track's sleepers, rails, rack and funicular cable, and the treads,
// risers and cheeks of steps. They draw in pieces of their own (mesh.ts), and cast.
import { PATH, RAIL_CODE, type Line } from '../terrain/bins';
import { PART, SLEEPER_TOP_M } from './parts';
import { lookWord, type PieceBuilder } from './piece';
import { CF, classOf, LOOK } from './section';
import type { Station } from './stations';
import { countOf, type Run } from './sweep';

/** The piece of the station where a node owns it. */
type Pick = (s: Station) => PieceBuilder | undefined;

/** A rail's cross-section from right to left: metres across from its middle and up from the sleeper's top, for a rail 0.125 m high. */
const RAIL: [number, number][] = [[-0.05, 0], [-0.05, 0.012], [-0.009, 0.028], [-0.009, 0.088], [-0.03, 0.097], [-0.031, 0.125], [0.031, 0.125], [0.03, 0.097], [0.009, 0.088], [0.009, 0.028], [0.05, 0.012], [0.05, 0]];
/** The edge of RAIL that is the rail's running head. */
const HEAD_EDGE = 5;
/** A ladder rack: two plates 0.16 m apart, with their rungs between, which its look draws on its top. */
const RACK: [number, number][] = [[-0.08, 0], [-0.08, 0.09], [0.08, 0.09], [0.08, 0]];
/** A funicular's haul cable: a band 3 cm wide over its rollers, one every ROLLER_M. */
const CABLE: [number, number][] = [[-0.015, 0.06], [-0.015, 0.09], [0.015, 0.09], [0.015, 0.06]], ROLLER_M = 8;
/** A riser of steps, the least and most tread, and how far a tread lies over the core. */
const RISE_M = 0.16, TREAD_MIN_M = 0.26, TREAD_MAX_M = 1.6, TREAD_LIFT_M = 0.04;

interface Gauge { rail: number; scale: number; rack: boolean; cable: boolean; sleeper: number }

/** A track's look: the middle of each rail from the track's middle, the rail's size, a rack, a funicular's cable, and its sleeper. */
function gaugeOf(line: Line): Gauge {
    const narrow = ((line.code - RAIL_CODE) & 1) !== 0, cls = classOf(line), wood = cls === 'funicular' || cls === 'rack';
    return {
        rail: narrow ? 0.53 : 0.7525, scale: narrow ? 1 : 1.2, rack: line.code >= RAIL_CODE + 2 || cls === 'rack', cable: cls === 'funicular',
        sleeper: narrow ? (wood ? PART.TIMBER_NARROW : PART.SLEEPER_NARROW) : wood ? PART.TIMBER : PART.SLEEPER,
    };
}

/** A station's point `a` metres to its left, `up` metres over the way's plane. */
function vertex(b: PieceBuilder, run: Run, s: Station, a: number, up: number, mat: number, along = countOf(run, s), flags = 0): number {
    return b.vertex(s.x + s.ax * a, s.y + s.ay * a, s.z + s.bank * a + run.lift + up, lookWord(mat, run.line.code, flags, 0, 0), a, along);
}

/** The quad of four vertices counter-clockwise seen from outside. Its faces take the first one's look. */
const face = (b: PieceBuilder, [p, q, r, s]: number[]) => b.near.push(p, q, r, p, r, s);

/** A cross-section (metres across, up), from right to left, swept along the stations that a node owns; the face from point k takes material mat(k). */
function swept(run: Run, pick: Pick, shape: [number, number][], up: number, mat: (k: number) => number): void {
    const { stations } = run, ring = (b: PieceBuilder, s: Station) => shape.map(([a, rise], k) => vertex(b, run, s, a, up + rise, mat(k)));
    let last: { b: PieceBuilder; j: number; ring: number[] } | undefined;
    for (let j = 0; j + 1 < stations.length; j++) {
        const b = pick(stations[j]);
        if (!b) continue;
        const from = last?.b === b && last.j === j ? last.ring : ring(b, stations[j]), to = ring(b, stations[j + 1]);
        for (let k = 0; k + 1 < shape.length; k++) face(b, [from[k], to[k], to[k + 1], from[k + 1]]);
        last = { b, j: j + 1, ring: to };
    }
}

/** Places a part at each `at` of the owned segments: `at` gives the shares along a segment. */
function placed(run: Run, pick: Pick, part: number, at: (s: Station, t: Station) => number[]): void {
    const { stations, lift } = run;
    for (let j = 0; j + 1 < stations.length; j++) {
        const s = stations[j], t = stations[j + 1], b = pick(s), grade = (t.z - s.z) / Math.max(t.along - s.along, 1e-6);
        for (const u of b ? at(s, t) : []) {
            const mix = (p: number, q: number) => p + (q - p) * u;
            b!.place(part, mix(s.x, t.x), mix(s.y, t.y), mix(s.z, t.z) + lift, Math.atan2(-mix(s.ay, t.ay), mix(s.ax, t.ax)), grade);
        }
    }
}

/** A track's sleepers at each whole count, its rails, and a rack or a cable on its rollers. */
function track(run: Run, pick: Pick): void {
    const g = gaugeOf(run.line);
    placed(run, pick, g.sleeper, (s, t) => {
        const [lo, hi] = [Math.min(s.sleepers!, t.sleepers!), Math.max(s.sleepers!, t.sleepers!)], out: number[] = [];
        for (let k = Math.ceil(lo); k < hi; k++) out.push((k - s.sleepers!) / (t.sleepers! - s.sleepers!));
        return out;
    });
    for (const side of [1, -1]) swept(run, pick, RAIL.map(([a, up]) => [side * g.rail + a, up * g.scale]), SLEEPER_TOP_M, k => (k === HEAD_EDGE ? LOOK.RAIL_HEAD : LOOK.RAIL));
    if (g.rack) swept(run, pick, RACK, SLEEPER_TOP_M, () => LOOK.RACK);
    if (g.cable) {
        swept(run, pick, CABLE, SLEEPER_TOP_M, () => LOOK.CABLE);
        placed(run, pick, PART.ROLLER, (s, t) => {
            const out: number[] = [];
            for (let a = Math.ceil(s.along / ROLLER_M) * ROLLER_M; a < t.along; a += ROLLER_M) out.push((a - s.along) / (t.along - s.along));
            return out;
        });
    }
}

/** The station `along` metres on a line. */
function stationAt(stations: Station[], along: number): Station {
    let j = 0;
    while (j + 2 < stations.length && stations[j + 1].along <= along) j++;
    const s = stations[j], t = stations[j + 1], u = Math.max(0, Math.min(1, (along - s.along) / Math.max(t.along - s.along, 1e-6)));
    const mix = (p: number, q: number) => p + (q - p) * u;
    return { ...s, x: mix(s.x, t.x), y: mix(s.y, t.y), z: mix(s.z, t.z), ax: mix(s.ax, t.ax), ay: mix(s.ay, t.ay), bank: 0, along };
}

/**
 * Steps along a line: treads about RISE_M apart in height, each level at the higher end of its stretch, so
 * the flight stands on the sloped core; risers face down the flight; cheeks close the sides down under the
 * drawn ground, which the shader reads, so a flight across a slope stands on it and never over a gap.
 */
function steps(run: Run, pick: Pick): void {
    const { line, stations } = run, total = stations[stations.length - 1].along, rise = Math.abs(stations[stations.length - 1].z - stations[0].z);
    if (total < 0.5) return;
    const count = Math.max(1, Math.round(rise / RISE_M)), tread = Math.min(TREAD_MAX_M, Math.max(TREAD_MIN_M, total / count));
    const n = Math.max(1, Math.round(total / tread)), half = Math.max(0.6, Math.min(line.h * 0.8, 1.4));
    let prev = -Infinity;
    for (let k = 0; k < n; k++) {
        const a0 = (k * total) / n, a1 = ((k + 1) * total) / n, s = stationAt(stations, a0), t = stationAt(stations, a1), b = pick(s);
        const level = Math.max(s.z, t.z) + TREAD_LIFT_M, at = (p: Station, a: number, z: number, mat: number, along: number, flags = 0) => vertex(b!, run, { ...p, z }, a, 0, mat, along, flags);
        if (b) {
            // A tread's `along` is metres back from its nosing, at its start going up the line, else at its end.
            const up = s.z <= t.z, ns = up ? 0 : a1 - a0, nt = up ? a1 - a0 : 0;
            face(b, [at(s, -half, level, LOOK.TREAD, ns), at(t, -half, level, LOOK.TREAD, nt), at(t, half, level, LOOK.TREAD, nt), at(s, half, level, LOOK.TREAD, ns)]);
            if (k > 0 && Math.abs(level - prev) > 0.01) {
                const lo = Math.min(level, prev), hi = Math.max(level, prev), [right, left] = level > prev ? [half, -half] : [-half, half];
                face(b, [at(s, right, lo, LOOK.RISER, a0), at(s, left, lo, LOOK.RISER, a0), at(s, left, hi, LOOK.RISER, a0), at(s, right, hi, LOOK.RISER, a0)]);
            }
            for (const [from, to, a] of [[s, t, -half], [t, s, half]] as const) face(b, [at(from, a, level, LOOK.RISER, a0), at(from, a, from.z, LOOK.RISER, a0, CF.MESH), at(to, a, to.z, LOOK.RISER, a1, CF.MESH), at(to, a, level, LOOK.RISER, a1)]);
        }
        prev = level;
    }
}

/** What stands on a line, in the pieces that `pick` gives. */
export function tracks(run: Run, pick: Pick): void {
    const { line } = run;
    if (line.code >= RAIL_CODE && line.sleepers) track(run, pick);
    else if (line.kind === PATH && classOf(line) === 'steps') steps(run, pick);
}
