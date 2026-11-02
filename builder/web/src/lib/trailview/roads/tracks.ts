// What stands on a way near the eye: a track's sleepers, rails, rack and funicular cable, the treads, risers
// and cheeks of steps in a town, and the logs of steps on a trail. They draw in pieces of their own (mesh.ts),
// and cast.
import { PATH, RAIL_CODE, type Line } from '../terrain/bins';
import { LOG_HALF_M, NARROW_GAUGE_M, PART, RAIL_GAUGE_M, SLEEPER_TOP_M } from './parts';
import { lookWord, type PieceBuilder } from './piece';
import { hash, v3 } from '../models/shape';
import { CF, classOf, frays, LOOK, lookOf } from './section';
import { CUT, stationAt, type Station } from './stations';
import { countOf, type Run } from './sweep';

/** The piece of a station where a node owns it. */
type PieceOf = (s: Station) => PieceBuilder | undefined;

/** Whether a road stops the way from a station on. */
const stopped = ({ cut = 0 }: Station) => (cut & CUT.ROAD) !== 0;

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
/** A trail's logs: one each LOG_RISE_M of height, never nearer than LOG_MIN_M along the way. */
const LOG_RISE_M = 0.2, LOG_MIN_M = 0.9;
/** A log's turn in radians and its shift along the way, at most, and the seed of their hash. */
const LOG_TURN = 0.07, LOG_SHIFT_M = 0.06, LOG_SEED = 31;
/** A trail's tread at most, and how far a gravel path's edge frays into the ground: as trail_look and gravel_look of looks.wgsl. */
const TRAIL_TREAD_M = 0.35, GRAVEL_FRAY_M = 0.15;

interface Gauge { rail: number; scale: number; rack: boolean; cable: boolean; sleeper: number }

/** A track's look: the middle of each rail from the track's middle, the rail's size, a rack, a funicular's cable, and its sleeper. */
function gaugeOf(line: Line): Gauge {
    const narrow = ((line.code - RAIL_CODE) & 1) !== 0, cls = classOf(line), wood = cls === 'funicular' || cls === 'rack';
    return {
        rail: narrow ? NARROW_GAUGE_M : RAIL_GAUGE_M, scale: narrow ? 1 : 1.2, rack: line.code >= RAIL_CODE + 2 || cls === 'rack', cable: cls === 'funicular',
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
function swept(run: Run, pieceOf: PieceOf, shape: [number, number][], up: number, mat: (k: number) => number): void {
    const { stations } = run, ring = (b: PieceBuilder, s: Station) => shape.map(([a, rise], k) => vertex(b, run, s, a, up + rise, mat(k)));
    let last: { b: PieceBuilder; j: number; ring: number[] } | undefined;
    for (let j = 0; j + 1 < stations.length; j++) {
        const b = pieceOf(stations[j]);
        if (!b) continue;
        const from = last?.b === b && last.j === j ? last.ring : ring(b, stations[j]), to = ring(b, stations[j + 1]);
        for (let k = 0; k + 1 < shape.length; k++) face(b, [from[k], to[k], to[k + 1], from[k + 1]]);
        last = { b, j: j + 1, ring: to };
    }
}

/** Places a part at each `at` of the owned segments: `at` gives the shares along a segment. */
function placed(run: Run, pieceOf: PieceOf, part: number, at: (s: Station, t: Station) => number[]): void {
    const { stations, lift } = run;
    for (let j = 0; j + 1 < stations.length; j++) {
        const s = stations[j], t = stations[j + 1], b = pieceOf(s), grade = (t.z - s.z) / Math.max(t.along - s.along, 1e-6);
        for (const u of b ? at(s, t) : []) {
            const mix = (p: number, q: number) => p + (q - p) * u;
            b!.place(part, mix(s.x, t.x), mix(s.y, t.y), mix(s.z, t.z) + lift, Math.atan2(-mix(s.ay, t.ay), mix(s.ax, t.ax)), grade);
        }
    }
}

/** A track's sleepers at each whole count, its rails, and a rack or a cable on its rollers. */
function track(run: Run, pieceOf: PieceOf): void {
    const g = gaugeOf(run.line);
    placed(run, pieceOf, g.sleeper, (s, t) => {
        const [lo, hi] = [Math.min(s.sleepers!, t.sleepers!), Math.max(s.sleepers!, t.sleepers!)], out: number[] = [];
        for (let k = Math.ceil(lo); k < hi; k++) out.push((k - s.sleepers!) / (t.sleepers! - s.sleepers!));
        return out;
    });
    for (const side of [1, -1]) swept(run, pieceOf, RAIL.map(([a, up]) => [side * g.rail + a, up * g.scale]), SLEEPER_TOP_M, k => (k === HEAD_EDGE ? LOOK.RAIL_HEAD : LOOK.RAIL));
    if (g.rack) swept(run, pieceOf, RACK, SLEEPER_TOP_M, () => LOOK.RACK);
    if (g.cable) {
        swept(run, pieceOf, CABLE, SLEEPER_TOP_M, () => LOOK.CABLE);
        placed(run, pieceOf, PART.ROLLER, (s, t) => {
            const out: number[] = [];
            for (let a = Math.ceil(s.along / ROLLER_M) * ROLLER_M; a < t.along; a += ROLLER_M) out.push((a - s.along) / (t.along - s.along));
            return out;
        });
    }
}

/**
 * Steps in a town along a line: treads about RISE_M apart in height, each level at the higher end of its
 * stretch, so the flight stands on the sloped core; risers face down the flight; cheeks close the sides down under the
 * drawn ground, which the shader reads, so a flight across a slope stands on it and never over a gap.
 */
function steps(run: Run, pieceOf: PieceOf): void {
    const { line, stations } = run, total = stations[stations.length - 1].along, rise = Math.abs(stations[stations.length - 1].z - stations[0].z);
    if (total < 0.5) return;
    const count = Math.max(1, Math.round(rise / RISE_M)), tread = Math.min(TREAD_MAX_M, Math.max(TREAD_MIN_M, total / count));
    const n = Math.max(1, Math.round(total / tread)), half = Math.max(0.6, Math.min(line.h * 0.8, 1.4));
    // NaN: no riser before a flight's first tread or after a road.
    let prev = NaN;
    for (let k = 0; k < n; k++) {
        const a0 = (k * total) / n, a1 = ((k + 1) * total) / n, s = stationAt(stations, a0), t = stationAt(stations, a1), b = pieceOf(s);
        if (stopped(s) || stopped(t)) {
            prev = NaN;
            continue;
        }
        const level = Math.max(s.z, t.z) + TREAD_LIFT_M, at = (p: Station, a: number, z: number, mat: number, along: number, flags = 0) => vertex(b!, run, { ...p, z, bank: 0 }, a, 0, mat, along, flags);
        if (b) {
            // A tread's `along` is metres back from its nosing, at its start going up the line, else at its end.
            const up = s.z <= t.z, ns = up ? 0 : a1 - a0, nt = up ? a1 - a0 : 0;
            face(b, [at(s, -half, level, LOOK.TREAD, ns), at(t, -half, level, LOOK.TREAD, nt), at(t, half, level, LOOK.TREAD, nt), at(s, half, level, LOOK.TREAD, ns)]);
            if (Math.abs(level - prev) > 0.01) {
                const lo = Math.min(level, prev), hi = Math.max(level, prev), [right, left] = level > prev ? [half, -half] : [-half, half];
                face(b, [at(s, right, lo, LOOK.RISER, a0), at(s, left, lo, LOOK.RISER, a0), at(s, left, hi, LOOK.RISER, a0), at(s, right, hi, LOOK.RISER, a0)]);
            }
            for (const [from, to, a] of [[s, t, -half], [t, s, half]] as const) face(b, [at(from, a, level, LOOK.RISER, a0), at(from, a, from.z, LOOK.RISER, a0, CF.MESH), at(to, a, to.z, LOOK.RISER, a1, CF.MESH), at(to, a, level, LOOK.RISER, a1)]);
        }
        prev = level;
    }
}

/** A log at a crossing of height level k of segment j, `a` metres along the line. */
interface Log { j: number; a: number; k: number }

/**
 * The logs of a flight on a trail, the same in each tile that holds it and whichever way OSM draws it: where the
 * way crosses a height of k·LOG_RISE_M, for each k that is a multiple of the m that the grade there needs to keep
 * logs LOG_MIN_M apart. Two crossings of one height nearer than LOG_MIN_M, at a crest or in a dip, lay no log; a
 * flight that crosses a height but keeps no log lays one at the height nearest its middle.
 */
function logsOf(stations: Station[]): Log[] {
    const all: (Log & { m: number })[] = [];
    for (let j = 0; j + 1 < stations.length; j++) {
        const s = stations[j], t = stations[j + 1], grade = (t.z - s.z) / (t.along - s.along);
        if (!(Math.abs(grade) > 1e-6)) continue;
        const m = Math.max(1, Math.ceil((LOG_MIN_M * Math.abs(grade)) / LOG_RISE_M - 1e-6));
        for (let k = Math.ceil(Math.min(s.z, t.z) / LOG_RISE_M); k * LOG_RISE_M <= Math.max(s.z, t.z); k++) {
            const a = s.along + (k * LOG_RISE_M - s.z) / grade;
            // A crossing at a station belongs to the segment after it, and the line's end to its last segment.
            if (a >= s.along && (a < t.along || (j + 2 === stations.length && a <= t.along))) all.push({ j, a, k, m });
        }
    }
    const lone = all.filter(log => !all.some(other => other !== log && other.k === log.k && Math.abs(other.a - log.a) < LOG_MIN_M));
    const kept = lone.filter(({ k, m }) => ((k % m) + m) % m === 0);
    if (kept.length || !lone.length) return kept;
    const ks = lone.map(({ k }) => k), middle = (Math.min(...ks) + Math.max(...ks)) / 2, off = ({ k }: Log) => Math.abs(k - middle) + k * 1e-9;
    return [lone.reduce((best, log) => (off(log) < off(best) ? log : best))];
}

/** The half-width of a trail's tread, where its look is soil, not grass: as trail_look and gravel_look of looks.wgsl. */
const treadOf = (line: Line) => (lookOf(line) === LOOK.TRAIL ? Math.min(TRAIL_TREAD_M, line.h - 0.05) : line.h - GRAVEL_FRAY_M);

/**
 * Steps on a trail: the way's own sloped core is the tread, and a log lies across it at each of logsOf. A hash of
 * its height level gives each log its length within the tread, a small turn and a small shift uphill or down.
 */
function logs(run: Run, pieceOf: PieceOf): void {
    const { stations, lift, line } = run, fits = Math.max(1, LOG_HALF_M.filter(half => half <= treadOf(line)).length);
    for (const { j, a, k } of logsOf(stations)) {
        const s = stations[j], t = stations[j + 1], b = pieceOf(s), grade = (t.z - s.z) / (t.along - s.along);
        if (!b) continue;
        const random = (salt: number) => hash(v3(k, salt), LOG_SEED), u = (a + LOG_SHIFT_M * (2 * random(1) - 1) * Math.sign(grade) - s.along) / (t.along - s.along);
        const mix = (p: number, q: number) => p + (q - p) * u, part = PART.LOG + Math.max(0, fits - 1 - Math.floor(2 * random(2)));
        b.place(part, mix(s.x, t.x), mix(s.y, t.y), mix(s.z, t.z) + lift, Math.atan2(-mix(s.ay, t.ay), mix(s.ax, t.ax)) + LOG_TURN * (2 * random(3) - 1), grade);
    }
}

/** What stands on a line, up to the roads that stop it, in the pieces that `pieceOf` gives. */
export function tracks(run: Run, detailAt: PieceOf): void {
    const { line } = run, pieceOf: PieceOf = s => (stopped(s) ? undefined : detailAt(s));
    if (line.code >= RAIL_CODE && line.sleepers) track(run, pieceOf);
    else if (line.kind === PATH && classOf(line) === 'steps') (frays(line) ? logs : steps)(run, pieceOf);
}
