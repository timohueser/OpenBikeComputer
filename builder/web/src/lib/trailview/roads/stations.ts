// The stations along a line: points on its curve, a fresh sampling of the Hermite spans that the stamp
// cuts into chords, each with its height and its frame across the road.
import { crossSlope, metresPerUnit, mitre, PATH, shoulderOf, type Line } from '../terrain/bins';
import { pointOn, spanOf } from '../stream/curve';
import { pivotsOf, type Pivot } from '../terrain/pivots';
import type { Paint } from './marks';

/** The longest step between two stations: the band anchors to the ground at each. */
export const STEP_M = 2;
/**
 * The most that the curve turns between two stations, so offset edges stay smooth; but stations lie
 * MIN_STEP_M apart at least: a vertex rounds to a share of a world unit, which tilts the face of a short
 * step, and a flat-shaded kerb then flickers from light to shade.
 */
const MAX_TURN = 2 * Math.PI / 180, MIN_STEP_M = 1;
/** Curve samples are this far apart. */
const FINE_M = 0.25;
/** The most that two stations of a fan turn about its pivot. */
const FAN_TURN = (6 * Math.PI) / 180;

/** A band may reach this share of the way to a tight bend's middle: past it, its faces fold into spikes. */
const FOLD = 0.9;
/** The far range keeps a station every this many metres at most, and where a chord would leave the skipped ones by more than these. */
const FAR_STEP_M = 10, FAR_SAG_M = 0.1, FAR_SAG_Z_M = 0.05;

/** Two stations this near are one. */
export const SAME_M = 0.01;
/** What a junction leaves out of the faces from a station to the next: its carriageway, its left side, its right side. */
export const CUT = { ROAD: 1, LEFT: 2, RIGHT: 4, ALL: 7 } as const;

export interface Station {
    x: number;
    y: number;
    z: number;
    /** Across to the left, world units per metre. */
    ax: number;
    ay: number;
    /** Metres from the line's start. */
    along: number;
    /** The segment that the station lies on, for the per-segment data. */
    v: number;
    /** The rise per metre to the left: a bend's banking or a junction's plane, as the stamp holds it. */
    bank: number;
    /** The carriageway's half-width here. */
    h: number;
    /** A track's count of sleepers from its start. */
    sleepers?: number;
    /** The bits of CUT to the next station. */
    cut?: number;
    /** The paint in place of the carriageway to the next station. */
    paint?: Paint;
    /** The share of its kerb's drop on its left and its right. */
    drop?: [number, number];
    /** Near a sidewalk's end, the share that its kerb and slab lie lowered to the road, on its left and its right. */
    end?: [number, number];
    /** In a tight bend's fan, its inner cross-section, which points at the bend's pivot. */
    rung?: Rung;
}

/**
 * A cross-section that points at a pivot: its side, world units per metre toward the pivot, the metres to the core's edge
 * on the pivot's arc, the arc's radius, and the core's half-width that those metres stretch: the carriageway and its shoulder.
 */
export interface Rung { side: 1 | -1; ux: number; uy: number; length: number; r: number; core: number }

/**
 * The most that one face across a carriageway's inner half turns its slope, the most faces across that half, and the
 * least share of the distance from where its cross-sections meet that two rows lie apart at its edge: the two faces of a
 * quad there differ in slope by about that share. A path's rows keep PATH_ROW_M apart, as closer ones leave thin faces on
 * its steep switchbacks where the next cross-section has fewer: chosen against the torn faces of the fixture.
 */
const ROW_TURN = (2 * Math.PI) / 180, ROWS_MOST = 12, ROW_SHARE = 0.15, PATH_ROW_M = 0.3;

/**
 * The faces across the inner half of a carriageway whose cross-sections meet `middle` metres from its middle and `edge`
 * from its edge, on a way that rises at `grade`: along the strip between two cross-sections the slope grows from the middle
 * to the edge by their ratio, and each face takes at most ROW_TURN of that, in a geometric series of distances from where
 * they meet.
 */
export function facesOf(line: Line, grade: number, middle: number, edge: number): number {
    const least = line.kind === PATH ? PATH_ROW_M : ROW_SHARE * edge;
    const turn = Math.ceil((Math.atan((grade * middle) / edge) - Math.atan(grade)) / ROW_TURN), room = Math.floor(Math.log(middle / edge) / Math.log(1 + least / edge));
    return Math.max(1, Math.min(ROWS_MOST, room, turn));
}

/**
 * World x, y and the metres along its cross-section of a point `a` metres across station s, signed. On a fan's side the
 * cross-section runs along the rung: the core stretched out to the pivot's arc, and the parts past its edge at their own widths.
 */
export function across(s: Station, a: number): [number, number, number] {
    const rung = s.rung;
    if (!rung || a * rung.side <= 0) return [s.x + s.ax * a, s.y + s.ay * a, a];
    const out = Math.abs(a), d = out <= rung.core ? out * rung.length / rung.core : rung.length + out - rung.core;
    return [s.x + rung.ux * d, s.y + rung.uy * d, rung.side * d];
}

/**
 * Gives the stations of each pivot's fan their rung: those from its first vertex to its last, whose rungs point at the
 * pivot and end on its arc. The core never narrows, as a pivot lies its half-width and r or more from the line. The segment
 * before the fan and the one after hold stations only on their vertices (stationsOf), so each face of the sweep lies in
 * a quad of the stamp, between the rungs that it holds.
 */
export function rungsOf(line: Line, stations: Station[], pivots: Pivot[]): void {
    /** The station on vertex v: stationsOf places one on each vertex of a fan. */
    const on = (v: number) => stations.findIndex(({ x, y }) => x === line.xy[2 * v] && y === line.xy[2 * v + 1]);
    for (const pivot of pivots) {
        const [from, to] = [on(pivot.first), on(pivot.last)];
        for (let j = from; from >= 0 && j <= to; j++) {
            const s = stations[j], m = metresPerUnit(s.y), dx = pivot.x - s.x, dy = pivot.y - s.y, d = Math.hypot(dx, dy) * m, core = s.h + shoulderOf(line);
            s.rung = { side: pivot.side, ux: dx / d, uy: dy / d, length: Math.max(core, d - pivot.r), r: pivot.r, core };
        }
    }
}

/** The metres along its stroke of a point `along` metres from a line's start, or those metres where it has no stroke. */
export const strokeMetres = ({ stroke }: Line, along: number) => (stroke ? stroke.at + stroke.sign * along : along);

const angle = (a: number) => Math.atan2(Math.sin(a), Math.cos(a));

/**
 * The stations of a line, in order; `half` gives the half-width at a vertex. Each span between two vertices
 * of the tile is sampled alone, from a station on its first vertex, so two tiles that hold a span agree.
 */
export function stationsOf(line: Line, half: (v: number) => number): Station[] {
    // A fan's cross-sections are those of the stamp: a station on each vertex of the fan and the one on each side, and none
    // inside the segment before it or the one after, whose quads in the stamp turn from the one end's cross-section to the other's.
    const pivots = pivotsOf(line, half), fanned = (v: number) => pivots.some(({ first, last }) => v >= first - 1 && v <= last + 1);
    const bounding = (v: number) => pivots.some(({ first, last }) => v === first - 1 || v === last), inFan = (v: number) => pivots.find(({ first, last }) => v >= first && v < last);
    const n = line.z.length, out: Station[] = [], tangent = (v: number) => { const [x, y] = mitre(line, v); return Math.atan2(y, x); };
    let along = 0, run = 0, kept = tangent(0), before = kept;
    const place = (v: number, u: number, metres: number, x: number, y: number, z: number, t: number) => {
        const m = metresPerUnit(y), [h0, h1] = [half(v), half(v + 1)], sleepers = line.sleepers;
        out.push({
            x, y, z, ax: Math.sin(t) / m, ay: -Math.cos(t) / m, along, v, bank: -crossSlope(line, v, u * metres, (1 - u) * metres), h: h0 + (h1 - h0) * u,
            sleepers: sleepers && sleepers[v] + (sleepers[v + 1] - sleepers[v]) * u,
        });
    };
    for (let v = 0; v + 1 < n; v++) {
        const [ax, ay, bx, by] = [line.xy[2 * v], line.xy[2 * v + 1], line.xy[2 * v + 2], line.xy[2 * v + 3]], [z0, z1] = [line.z[v], line.z[v + 1]];
        const dx = bx - ax, dy = by - ay, metres = Math.hypot(dx, dy) * metresPerUnit((ay + by) / 2), rise = (z1 - z0) / (metres || 1);
        const [t0, t1] = [tangent(v), tangent(v + 1)], chord = Math.atan2(dy, dx), straight = Math.abs(angle(t0 - chord)) + Math.abs(angle(t1 - chord)) < 1e-3;
        const span = spanOf(dx, dy, t0, t1, z0, z1, line.plan?.grade[v] ?? rise, line.plan?.grade[v + 1] ?? rise, metres);
        // A straight segment of a fan still turns about its pivot: it is sampled as finely as a bend.
        const fan = inFan(v), fine = straight && !fan ? Math.max(1, Math.ceil(metres / STEP_M)) : Math.max(8, Math.ceil(metres / FINE_M)), last = metres / fine;
        if (v === 0) place(0, 0, metres, ax, ay, z0, t0);
        for (let k = 1, x = ax, y = ay; k <= fine; k++) {
            const u = k / fine, c = k < fine ? pointOn(span, u) : undefined, nx = c ? ax + c.x : bx, ny = c ? ay + c.y : by, t = c?.tangent ?? t1, d = c ? Math.hypot(nx - x, ny - y) * metresPerUnit(ny) : last;
            along += d;
            run += d;
            [x, y] = [nx, ny];
            // A line ends on a station, as does each vertex of the tile, and each side of a change of its sidewalks has one.
            const forced = k === fine && (v + 2 === n || (line.anchors?.[v + 1] ?? true) || line.sidewalks?.[v] !== line.sidewalks?.[v + 1] || fanned(v + 1));
            const turn = Math.abs(angle(t - kept)) + Math.abs(angle(t - before)), prev = out[out.length - 1];
            before = t;
            // In a fan, the faces about the pivot turn by FAN_TURN at most, so the steep ground at the inner edge shades smoothly.
            const about = fan && Math.abs(angle(Math.atan2(ny - fan.y, nx - fan.x) - Math.atan2(prev.y - fan.y, prev.x - fan.x)));
            if (!forced && (bounding(v) || (about ? about <= FAN_TURN || run + d <= FINE_M : run + d <= STEP_M && (run + d <= MIN_STEP_M || turn <= MAX_TURN)))) continue;
            run = 0;
            kept = before = t;
            place(v, u, metres, nx, ny, c?.z ?? z1, t);
        }
    }
    return out;
}

/** The index of the station nearest to metres `along`. */
export function stationIndex(stations: Station[], along: number): number {
    let lo = 0, hi = stations.length - 1;
    while (lo < hi) {
        const middle = (lo + hi) >> 1;
        if (stations[middle].along < along) lo = middle + 1;
        else hi = middle;
    }
    return lo > 0 && along - stations[lo - 1].along < stations[lo].along - along ? lo - 1 : lo;
}

/** The station nearest to metres `along`: where the plan asked for one, within SAME_M of them. */
export const stationOn = (stations: Station[], along: number) => stations[stationIndex(stations, along)];

/** A station a share u of the way from s to t. */
export function between(s: Station, t: Station, u: number): Station {
    const mix = (a: number, b: number) => a + (b - a) * u;
    return {
        x: mix(s.x, t.x), y: mix(s.y, t.y), z: mix(s.z, t.z), ax: mix(s.ax, t.ax), ay: mix(s.ay, t.ay), along: mix(s.along, t.along), v: u < 1 ? s.v : t.v, bank: mix(s.bank, t.bank), h: mix(s.h, t.h),
        sleepers: s.sleepers === undefined || t.sleepers === undefined ? undefined : mix(s.sleepers, t.sleepers),
    };
}

/** The station `metres` along stations of a line, at most at its ends. It keeps the cut of its segment. */
export function stationAt(stations: Station[], metres: number): Station {
    let j = 0;
    while (j + 2 < stations.length && stations[j + 1].along <= metres) j++;
    const s = stations[j], t = stations[j + 1];
    return { ...between(s, t, Math.max(0, Math.min(1, (metres - s.along) / Math.max(t.along - s.along, 1e-6)))), cut: s.cut };
}

/**
 * Of each station, the most metres across that its sides reach on its left and its right, where a tight
 * bend stops them. An offset that shortens a step to nothing folds; the ends of a step share its limit. A rung
 * reaches its pivot: the skirt draws all the ground that the fan's rungs enclose, which the way owns.
 */
export function reachOf(stations: Station[]): Float64Array {
    const out = new Float64Array(2 * stations.length).fill(Infinity);
    for (let j = 0; j + 1 < stations.length; j++) {
        foldOf(stations, j, j + 1).forEach((fold, side) => {
            out[2 * j + side] = Math.min(out[2 * j + side], FOLD * fold);
            out[2 * j + 2 + side] = Math.min(out[2 * j + 2 + side], FOLD * fold);
        });
    }
    stations.forEach(({ rung }, j) => {
        if (rung) out[2 * j + (rung.side > 0 ? 0 : 1)] = rung.core + rung.r;
    });
    return out;
}

/**
 * Of the steps from station j to k, the least metres across on the left and on the right where an offset shrinks to
 * nothing. A side whose rung points at a pivot at either end of a step does not fold there.
 */
export function foldOf(stations: Station[], j: number, k: number): [number, number] {
    const out: [number, number] = [Infinity, Infinity];
    for (let i = j; i < k; i++) {
        const s = stations[i], t = stations[i + 1], dx = t.x - s.x, dy = t.y - s.y, l = Math.hypot(dx, dy);
        const shorter = l > 0 ? ((t.ax - s.ax) * dx + (t.ay - s.ay) * dy) / l : 0, side = shorter < 0 ? 0 : 1;
        if (Math.abs(shorter) >= 1e-12 && ![s, t].some(({ rung }) => rung && (rung.side > 0 ? 0 : 1) === side)) out[side] = Math.min(out[side], l / Math.abs(shorter));
    }
    return out;
}

/** Whether a chord from station s to t stays within the far range's sags at station p. */
function holds(s: Station, t: Station, p: Station): boolean {
    const dx = t.x - s.x, dy = t.y - s.y, l2 = dx * dx + dy * dy || 1, u = ((p.x - s.x) * dx + (p.y - s.y) * dy) / l2;
    const off = Math.hypot(p.x - s.x - u * dx, p.y - s.y - u * dy) * metresPerUnit(p.y);
    return off <= FAR_SAG_M && Math.abs(s.z + (t.z - s.z) * u - p.z) <= FAR_SAG_Z_M;
}

/**
 * The indices of the stations that the far range keeps: the ends, each side of a change of the sidewalks,
 * each station that a junction cuts, whose edges its faces start from, each that bounds paint, the first station in each square of
 * `side` world units, and enough for its chords. A near range ends on that first station too, so the two
 * ranges meet on it whichever square draws which, and from it on each tile that holds the way keeps the same
 * stations.
 */
export function farStations(line: Line, stations: Station[], side: number): number[] {
    const square = ({ x, y }: Station) => Math.floor(x / side) * 2 ** 20 + Math.floor(y / side);
    const stays = (j: number) => line.sidewalks?.[stations[j].v] === line.sidewalks?.[stations[j + 1]?.v] && square(stations[j]) === square(stations[j - 1]) && !stations[j].cut && !stations[j - 1].cut && !stations[j].paint && !stations[j - 1].paint;
    const out = [0];
    for (let from = 0, j = 2; j <= stations.length; j++) {
        const last = j === stations.length, held = !last && stations[j].along - stations[from].along <= FAR_STEP_M && stays(j - 1);
        let fits = held;
        for (let k = from + 1; fits && k < j; k++) fits = holds(stations[from], stations[j], stations[k]);
        if (fits) continue;
        out.push(j - 1);
        from = j - 1;
    }
    return out;
}
