// PROTOTYPE road system (a study; never merges). The stations along a line: points on its smooth curve
// (a C1 cubic Hermite curve through its points with the fit's tangents and grades, curve.ts), at most
// STEP_M and MAX_TURN apart, each with its height, its unit vector across to the left in world units per
// metre, its metres from the start, its segment, its bank, its half-width and, on a track, its sleepers.
import { crossSlope, metresPerUnit, type Line } from '../terrain/bins';
import { bessel, hermite } from './curve';

/** The longest step between two stations: the band anchors to the drawn mesh at each. */
export const STEP_M = 2;
/**
 * The most that the curve turns between two stations, in radians: about a degree, so offset edges stay smooth;
 * but stations lie MIN_STEP_M apart at least: a vertex rounds to a whole world unit (2.5 cm here), which tilts
 * the face of a short step, and a kerb's flat-shaded face then flickers from light to shade.
 */
const MAX_TURN = 2 * Math.PI / 180, MIN_STEP_M = 1;

export interface Station {
    x: number;
    y: number;
    z: number;
    /** Across to the left, world units per metre. */
    ax: number;
    ay: number;
    along: number;
    /** The segment of the line that the station lies on, for the per-segment data (sidewalks). */
    v: number;
    /** The surface's slope across, rise per metre to the left: a bend's banking, or a junction's plane on a side road, as the stamp holds it (whose tilts rise to the right). */
    bank: number;
    /** The carriageway's half-width here: wider on a tight bend, tapered to a partner's at a joint. */
    h: number;
    sleepers?: number;
    /** The station's index along its line; none for a made-up station. */
    j?: number;
}

/** The unit tangents (two numbers per vertex) and grades of a line: the fit's, else the parabola through each vertex and its neighbours. */
function tangents(line: Line): { dir: number[]; grade: number[] } {
    const n = line.z.length;
    if (line.dir?.length === 2 * n && line.grade?.length === n) return { dir: line.dir, grade: line.grade };
    const s = [0];
    for (let v = 1; v < n; v++) s.push(s[v - 1] + Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * metresPerUnit(line.xy[2 * v + 1]));
    const dx = bessel(line.xy.filter((_, i) => i % 2 === 0), s), dy = bessel(line.xy.filter((_, i) => i % 2 === 1), s), dir: number[] = [];
    for (let v = 0; v < n; v++) {
        const l = Math.hypot(dx[v], dy[v]) || 1;
        dir.push(dx[v] / l, dy[v] / l);
    }
    return { dir, grade: Array.from(bessel(line.z, s)) };
}

/** The angle between two unit vectors. */
const turn = (ax: number, ay: number, bx: number, by: number) => Math.abs(Math.atan2(ax * by - ay * bx, ax * bx + ay * by));

/** The stations of a line, in order; `half` gives the half-width at a vertex. */
export function stationsOf(line: Line, step = STEP_M, half: (v: number) => number = v => line.h + (line.widen?.[v] ?? 0)): Station[] {
    const n = line.z.length, out: Station[] = [], { dir, grade } = tangents(line);
    let along = 0;
    for (let v = 0; v + 1 < n; v++) {
        const p0: [number, number] = [line.xy[2 * v], line.xy[2 * v + 1]], p1: [number, number] = [line.xy[2 * v + 2], line.xy[2 * v + 3]];
        const t0: [number, number] = [dir[2 * v], dir[2 * v + 1]], t1: [number, number] = [dir[2 * v + 2], dir[2 * v + 3]];
        const unit = metresPerUnit(p0[1]), chord = Math.hypot(p1[0] - p0[0], p1[1] - p0[1]) || 1, metres = chord * unit, h0 = half(v), h1 = half(v + 1);
        // Fine samples of the curve, about FINE_M apart (a straight segment needs none); a station before the
        // next sample would pass `step` metres, or MAX_TURN past MIN_STEP_M, from the last station.
        const cx = (p1[0] - p0[0]) / chord, cy = (p1[1] - p0[1]) / chord, straight = turn(t0[0], t0[1], cx, cy) + turn(cx, cy, t1[0], t1[1]) < 1e-3;
        const fine = straight ? Math.max(1, Math.ceil(metres / step)) : Math.max(8, Math.ceil(metres / FINE_M));
        const at = (u: number) => hermite(p0, p1, t0, t1, line.z[v], line.z[v + 1], grade[v], grade[v + 1], metres, u);
        let c = at(0), run = 0, kept = c, next = at(1 / fine);
        if (v === 0) out.push(station(line, v, 0, metres, c, 0, h0, h1, out.length));
        for (let k = 1; k <= fine; k++) {
            const d = Math.hypot(next.x - c.x, next.y - c.y) * unit;
            along += d;
            run += d;
            c = next;
            if (k < fine) {
                next = at((k + 1) / fine);
                const ahead = run + Math.hypot(next.x - c.x, next.y - c.y) * unit;
                if (ahead <= step && (ahead <= MIN_STEP_M || turn(kept.tx, kept.ty, next.tx, next.ty) <= MAX_TURN)) continue;
            }
            run = 0;
            kept = c;
            out.push(station(line, v, k / fine, metres, k === fine ? { ...c, x: p1[0], y: p1[1], z: line.z[v + 1] } : c, along, h0, h1, out.length));
        }
    }
    return out;
}

/** Curve samples are this far apart, in metres. */
const FINE_M = 0.25;

/** The station a share u along segment v of a line, at point c of its curve. */
function station(line: Line, v: number, u: number, metres: number, c: ReturnType<typeof hermite>, along: number, h0: number, h1: number, j: number): Station {
    const m = metresPerUnit(c.y), sl = line.sleepers;
    return {
        x: c.x, y: c.y, z: c.z, ax: c.ty / m, ay: -c.tx / m, along, v, bank: -crossSlope(line, v, u * metres, (1 - u) * metres), h: h0 + (h1 - h0) * u,
        sleepers: sl ? sl[v] + (sl[v + 1] - sl[v]) * u : undefined, j,
    };
}
