// The faces of a tile's junctions, from its lines and the junction plan of terrain/junction-plan.ts, which says what
// meets where. Between neighbouring ways a corner turns the kerb round an arc, and a way's side toward a corner stops at
// the arc's end. A side road's carriageway starts at its stop, in a mouth on the through road's edge, and a path or a
// driveway stops at its seam. This adds to each line the stations that the junctions need, and marks what the sweep
// leaves out to the junction's faces (junction-faces.ts), and where a sidewalk ends. A road bridge's end is a way of
// its junction: it stays through, and a side road's mouth runs from its deck's edge.
import { GUEST_INSIDE, WALK_END_M, WINDOW_M, type JunctionPlan, type Arm as PlanArm } from '../terrain/junction-plan';
import { PAVED_CODES, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { footOn, meetingsOf, outward, stationAlong, vertexAlong, type Arm, type Way } from './arms';
import { cornerOf, type Corner } from './corner';
import { isDeck, markDeck } from './decks';
import { jointsOf, mitre } from './joints';
import { giveWay, lowerAt, zebras, type Paint, type RoadMarks } from './marks';
import { CF, junctionFace, LOOK, sideOf } from './section';
import { between, CUT, SAME_M, stationIndex, type Station } from './stations';
import type { Run } from './sweep';

/** A side road's mouth: the way, metres along it where its whole carriageway starts, its corners by the side of its line, the through ways, and the metres along them that it spans from the junction. */
export interface Mouth { way: Way; far: number; left: Corner; right: Corner; through: Way[]; reach: number }

/** A path's or a driveway's stop on its hosts: the way, metres along it at the stop, the through ways, and whether its seam is a sidewalk's outer edge. */
export interface Seam { way: Way; at: number; hosts: Way[]; walk: boolean }

/** A junction's faces, its guests' seams, and the roads that end where only paths run on. */
export interface Junctions { corners: Corner[]; mouths: Mouth[]; seams: Seam[]; plain: Way[] }

/** A station that a junction asks for this near another is that one: a step of a few centimetres only twists its faces. */
const BREAK_SNAP_M = 0.05;
/** A lowered kerb ramps back up over this many metres past its stretch. Two paths out of a point, at this cosine or less, are one that runs on. */
const RAMP_M = 1.2, RUNS_ON_COS = -0.5;
/** A track's ruts wear in over this many metres past a side road's mouth. */
const RUTS_M = 2;
/** The most metres between the stations of a sidewalk's taper. */
const TAPER_STEP_M = 1;

/** Where a sidewalk ends on a side of a line: the metres along its stations, whether the sidewalk lies ahead of there (1) or behind (-1), and the metres of its taper. */
type End = { at: number; toward: 1 | -1; side: 1 | -1; length: number };
/**
 * What a junction does to a line: faces left out, stations it needs, its paint, its lowered kerbs and its sidewalks'
 * ends; and the stations of a sharp corner's join, which hold its side's line to a centimetre.
 */
type Marks = { cuts: { from: number; to: number; bits: number }[]; breaks: number[]; paints: { from: number; to: number; paint: Paint }[]; drops: { from: number; to: number; side: 1 | -1 }[]; ends: End[]; joins: number[] };
/** Marks a way from metres `from` to `to` with bits of CUT, and asks for a station at `to`. */
export type Mark = (way: { run: Run }, from: number, to: number, bits: number) => void;

/** The road arms with a corridor. */
const groundOf = (arms: Arm[]) => arms.filter((arm): arm is Way => !!arm.run);

/** Adds stations to a line at metres along it, each unless one lies within its snap. */
function addStations(run: Run, wanted: [number, number][]): void {
    const stations = run.stations.slice();
    for (const [along, snap] of wanted.sort((p, q) => p[0] - q[0])) {
        const j = stationIndex(stations, along), near = stations[j];
        if (Math.abs(near.along - along) < snap) continue;
        const k = near.along < along ? j : j - 1;
        if (k < 0 || k + 1 >= stations.length) continue;
        stations.splice(k + 1, 0, between(stations[k], stations[k + 1], (along - stations[k].along) / (stations[k + 1].along - stations[k].along)));
    }
    run.stations = stations;
}

/** The stations that a line's marks ask for, but for its sidewalks' ends: a sharp corner's join holds its line to a centimetre. */
const asked = ({ breaks, paints, drops, joins }: Marks): [number, number][] => [
    ...[...new Set([...breaks, ...paints.flatMap(({ from, to }) => [from, to]), ...drops.flatMap(({ from, to }) => [from - RAMP_M, from, to, to + RAMP_M])])].map((along): [number, number] => [along, BREAK_SNAP_M]),
    ...joins.map((along): [number, number] => [along, SAME_M]),
];

/** Sets a line's cuts, its paint, its lowered kerbs and the share of each sidewalk's end on its stations. */
function settle(run: Run, { cuts, paints, drops, ends }: Marks): void {
    const { stations } = run;
    for (let j = 0; j + 1 < stations.length; j++) {
        const middle = (stations[j].along + stations[j + 1].along) / 2;
        let bits = 0;
        for (const { from, to, bits: cut } of cuts) if (middle > Math.min(from, to) && middle < Math.max(from, to)) bits |= cut;
        stations[j].cut = bits || undefined;
        stations[j].paint = paints.find(({ from, to }) => middle > from && middle < to)?.paint;
    }
    if (drops.length || ends.length) stations.forEach((s, j) => {
        // A corner's ring stands on the full kerb of the station where the side's sweep stops.
        const cut = (bit: number) => ((s.cut ?? 0) | (stations[j - 1]?.cut ?? 0)) & bit;
        const drop = (side: 1 | -1, bit: number) => (cut(bit) ? 0 : Math.max(0, ...drops.filter(d => d.side === side).map(({ from, to }) => {
            const past = Math.max(from - s.along, s.along - to, 0) / RAMP_M;
            return past < 1 ? 1 - past * past * (3 - 2 * past) : 0;
        })));
        // The kerb's line runs straight from the carriageway's edge to the sidewalk's back edge.
        const end = (side: 1 | -1, bit: number) => (cut(bit) ? 0 : Math.max(0, ...ends.filter(e => e.side === side).map(({ at, toward, length }) => {
            const into = (s.along - at) * toward;
            return into > -SAME_M && into < length ? Math.min(1, 1 - into / length) : 0;
        })));
        s.drop = [drop(1, CUT.LEFT), drop(-1, CUT.RIGHT)];
        const shares: [number, number] = [end(1, CUT.LEFT), end(-1, CUT.RIGHT)];
        if (shares[0] || shares[1]) s.end = shares;
    });
}

/** The metres along a run's stations of each stretch of its line's segments that a path or a driveway has inside a host. */
function insideOf({ line, stations }: Run): [number, number][] {
    const out: [number, number][] = [], inside = line.inside ?? [];
    for (let s = 0; s < inside.length; s++) {
        if (inside[s] !== GUEST_INSIDE) continue;
        let e = s;
        while (e + 1 < inside.length && inside[e + 1] === GUEST_INSIDE) e++;
        out.push([vertexAlong(stations, s), vertexAlong(stations, e + 1)]);
        s = e;
    }
    return out;
}

/**
 * Plans the junctions of a tile's lines from its junction plan. Each line's stations take what the plan needs: every
 * tile that holds a junction within REACH_M reads the same WINDOW_M of its ways, so they all give it the same corners
 * and cuts.
 */
export function planJunctions(lines: Line[], runs: Run[], plan: JunctionPlan, crossings: { x: number; y: number }[] = []): Junctions {
    const out: Junctions = { corners: [], mouths: [], seams: [], plain: [] }, marks = new Map<Run, Marks>();
    const marksOf = (run: Run) => marks.get(run) ?? marks.set(run, { cuts: [], breaks: [], paints: [], drops: [], ends: [], joins: [] }).get(run)!;
    const roadMarks: RoadMarks = {
        paint: (run, from, to, paint) => marksOf(run).paints.push({ from: Math.min(from, to), to: Math.max(from, to), paint }),
        lower: (run, from, to, side) => {
            if (run.line.sidewalks?.some(walks => walks & (side > 0 ? LEFT : RIGHT))) marksOf(run).drops.push({ from, to, side });
        },
    };
    const mark: Mark = ({ run }, from, to, bits) => {
        const entry = marksOf(run);
        entry.breaks.push(to);
        if (bits) entry.cuts.push({ from, to, bits });
    };
    // What a host holds draws nothing of a path or a driveway inside it.
    for (const run of runs) for (const [from, to] of insideOf(run)) mark({ run }, from, to, CUT.ALL);
    for (const meeting of meetingsOf(lines, runs)) {
        const here = ({ x, y }: { x: number; y: number }) => x === meeting.x && y === meeting.y;
        const ways = groundOf(meeting.roads), all = [...ways, ...meeting.paths];
        // A line that meets itself here has two ways out with one step: the arm's is the one that `metres`, on the arm, lies nearest past.
        const wayOf = ({ line, step }: Pick<PlanArm, 'line' | 'step'>, metres: number) => {
            const ways = all.filter(arm => arm.line === line && arm.step === step), past = (arm: Way) => (metres - arm.along) * step;
            return ways.length < 2 ? ways[0] : ways.filter(arm => past(arm) > -SAME_M).reduce<Way | undefined>((best, arm) => (!best || past(arm) < past(best) ? arm : best), undefined);
        };
        const pair = meeting.roads.filter(({ through }) => through), faced = pair.map(({ line }) => line).filter(line => junctionFace(line));
        const road = faced.find(line => junctionFace(line)!.mat === LOOK.ASPHALT) ?? faced[0];
        const corners = plan.corners.filter(here).flatMap(corner => {
            const p = wayOf(corner.p, corner.ends[0]), q = wayOf(corner.q, corner.ends[1]);
            return p && q && road ? [cornerOf(corner, meeting, p, q, road)] : [];
        });
        const guests = plan.guests.filter(here);
        // Where a side road's edge meets its host behind the junction on one through way at a sharp corner, it leaves
        // the host on the other: there the other corner's cut of that host's side starts, so the host keeps its side up
        // to the road. A round corner's arc ends each side.
        const starts = new Map<Corner, number>();
        for (const guest of guests) {
            const way = guest.kind === 'side' ? wayOf(guest, guest.stop) : undefined, before = corners.find(({ q }) => q === way), after = corners.find(({ p }) => p === way);
            if (!way || !before || !after) continue;
            for (const [corner, other] of [[before, after], [after, before]]) if (!other.r) starts.set(corner, other.behind[other.p.through ? 0 : 1]);
        }
        for (const corner of corners) ([[corner.p, corner.sp, 0], [corner.q, corner.sq, 1]] as const).forEach(([way, side, end]) => {
            const from = way.along + (way.through ? way.step * Math.min(starts.get(corner) ?? 0, Math.abs(corner.ends[end] - way.along)) : 0);
            mark(way, from, corner.ends[end], side > 0 ? CUT.LEFT : CUT.RIGHT);
            if (way.through) mark(way, corner.meet[end], corner.meet[end], 0);
            if (from !== way.along) mark(way, from, from, 0);
        });
        out.corners.push(...corners);
        for (const guest of guests) {
            const way = wayOf(guest, guest.stop);
            if (guest.kind !== 'side' || !way) continue;
            const far = stationAlong(way.run.stations, way.line, guest.stop), before = corners.find(({ q }) => q === way), after = corners.find(({ p }) => p === way);
            if (!before || !after) continue;
            const reach = Math.max(...[before, after].map(({ p, q, meet }) => Math.abs(p.through ? meet[0] - p.along : meet[1] - q.along))), through = pair.filter((t): t is Way => !!t.run);
            out.mouths.push({ way, far, left: before.sq > 0 ? before : after, right: before.sq > 0 ? after : before, through, reach });
            // A track's ruts wear in past its mouth, whose faces fan from its section there.
            for (const s of way.run.stations) {
                const past = (s.along - far) * way.step;
                if (past > -SAME_M && past < RUTS_M) s.ruts = Math.min(s.ruts ?? 1, Math.max(0, past) / RUTS_M);
            }
            mark(way, way.along, far, CUT.ROAD);
            giveWay(way, far, through, roadMarks);
        }
        const hosts = ways.filter(({ through }) => through);
        // A road's end where only paths run on has a plain end.
        if (meeting.roads.length === 1 && ways.length === 1 && !!meeting.paths.length && PAVED_CODES.includes(ways[0].line.code) && !isDeck(ways[0].line)) out.plain.push(ways[0]);
        if (!hosts.length) continue;
        const seams = guests.flatMap(guest => {
            const way = guest.kind === 'side' ? undefined : wayOf(guest, guest.stop);
            return way ? [{ way, at: stationAlong(way.run.stations, way.line, guest.stop), hosts, walk: guest.walk, drive: guest.kind === 'driveway' }] : [];
        });
        // A driveway crosses the sidewalk, and a path that runs on crosses the road. A path that stops on the sidewalk leaves its kerb whole.
        for (const { way, walk, drive } of seams) if (walk && (drive || seams.some(other => other.way.u[0] * way.u[0] + other.way.u[1] * way.u[1] < RUNS_ON_COS))) lowerAt(way, ways, roadMarks);
        out.seams.push(...seams.map(({ way, at, hosts: on, walk }) => ({ way, at, hosts: on, walk })));
    }
    zebras(runs, crossings, roadMarks);
    for (const run of runs) if (isDeck(run.line)) markDeck(run, lines, mark);
    // A sharp corner's join holds each side on its own stations, with one where its hold passes a solid point of the side.
    const joins = out.corners.flatMap(({ p, q, sp, sq, ends, join }) => (join === undefined ? [] : [joinOf(p, sp, ends[0], q, sq, join), joinOf(q, sq, ends[1], p, sp, join)]));
    for (const { way, breaks } of joins) marksOf(way.run).joins.push(...breaks());
    for (const [run, entry] of marks) addStations(run, asked(entry));
    // Where a sidewalk ends, its kerb turns to its back edge and its slab narrows to nothing over WALK_END_M, or over half
    // a shorter piece, and short of a junction's cut, so a corner's ring meets a whole kerb. The faces from station j to
    // j + 1 take the sides of segment v of station j + 1, so a change lies at a station from 1 on; a station added
    // before the change would move it, so the ends are found on the stations that the junctions asked for.
    for (const run of runs) for (const side of [1, -1] as const) {
        const { stations, line } = run, bit = side > 0 ? CUT.LEFT : CUT.RIGHT, walked = (j: number) => ((line.sidewalks?.[stations[j].v] ?? 0) & (side > 0 ? LEFT : RIGHT)) !== 0;
        const changes: { at: number; toward: 1 | -1 }[] = [];
        for (let j = 1; j + 1 < stations.length; j++) if (walked(j) !== walked(j + 1)) changes.push({ at: stations[j].along, toward: walked(j) ? -1 : 1 });
        const cuts = marks.get(run)?.cuts.filter(({ bits }) => bits & bit) ?? [], last = stations[stations.length - 1].along;
        for (const { at, toward } of changes) {
            const ahead = (m: number) => (m - at) * toward;
            let length = Math.min(WALK_END_M, ahead(toward > 0 ? last : 0), ...changes.map(o => ahead(o.at)).filter(d => d > SAME_M).map(d => d / 2));
            for (const { from, to } of cuts) {
                const [near, far] = [Math.min(ahead(from), ahead(to)), Math.max(ahead(from), ahead(to))];
                if (far > SAME_M) length = Math.min(length, Math.max(0, near));
            }
            if (length > SAME_M) marksOf(run).ends.push({ at, toward, side, length });
        }
    }
    for (const [run, entry] of marks) {
        // A taper has a station every TAPER_STEP_M, so its kerb turns along a line of quads, and the fan from its point spans one step.
        addStations(run, entry.ends.flatMap(({ at, toward, length }) => {
            const n = Math.max(1, Math.ceil(length / TAPER_STEP_M));
            return Array.from({ length: n }, (_, k): [number, number] => [at + (toward * length * (k + 1)) / n, BREAK_SNAP_M]);
        }));
        settle(run, entry);
    }
    for (const { way: { run: { stations } }, which, holds } of joins) for (const { j, hold } of holds()) {
        stations[j].join ??= [Infinity, Infinity];
        stations[j].join![which] = Math.min(stations[j].join![which], hold);
    }
    mitre(jointsOf(lines, runs));
    return out;
}

/**
 * Where a side ends at a sharp corner, in metres from its middle. Its edge lies `h` out and its core `own` past that;
 * the other way's edge lies g0 + g1·d from the side's point d metres out, and its core `core` past it. The side keeps
 * the points that the other's carriageway and core do not hold, and of those that both cores hold the ones nearer its
 * own edge. Where the other's core would leave a hole in its own, it keeps all, and the other gives way.
 */
export function heldAt(h: number, own: number, core: number, g0: number, g1: number): number {
    const g = (d: number) => g0 + g1 * d;
    // Its edge lies in the other's carriageway, or on its edge with the section running into it.
    if (g(h) < -SAME_M || (g(h) <= SAME_M && g1 <= 0)) return h;
    // Up to the bisector the side's own edge is the nearer one.
    const bisector = g1 < 1 ? (h + g0) / (1 - g1) : Infinity;
    if (g(bisector) > core) return g1 < 0 ? (core - g0) / g1 : Infinity;
    if (g1 < 0) return bisector;
    return g1 > 0 && (core - g0) / g1 < h + own ? Infinity : bisector;
}

/** Breaks closer than this along a join hold its line to within a few centimetres. */
const JOIN_STEP_M = 0.05;

/**
 * A way's side past a sharp corner's meet, held per station up to the first where the side is whole (`heldAt`)
 * against the other way's edge and core at the foot of the station's edge on it. So the two ways' kerbs, sidewalks and
 * verges meet on the corner's bisector or where the narrower core ends, and neither lies over the other. `breaks` are
 * the metres along the way where the hold passes a solid point of the side or the side turns whole, so its faces end on the hold's line.
 */
function joinOf(way: Way, side: 1 | -1, end: number, other: Way, across: 1 | -1, reach: number) {
    const { run, step, unit } = way, list = outward(other, WINDOW_M + reach), { x: x0, y: y0 } = list[0];
    /** The hold at a station, Infinity where the side is whole, undefined where the other way faces it no more. */
    const holdAt = (s: Station) => {
        const { s: f, t, u: k, behind } = footOn(other, s.x + s.ax * side * s.h, s.y + s.ay * side * s.h, list), mix = (a: number, b: number) => a + (b - a) * k;
        // A point behind the other way's junction faces another of its arms, not this corner.
        if (behind > SAME_M || Math.abs(s.along - end) > reach) return undefined;
        // The other's edge on the corner's side at the foot, and its normal out of its middle, in metres about the list's start.
        const nx = across * mix(f.ax, t.ax) * unit, ny = across * mix(f.ay, t.ay) * unit, l = Math.hypot(nx, ny) || 1, n = [nx / l, ny / l], h = mix(f.h, t.h);
        const edge = [(mix(f.x, t.x) - x0) * unit + n[0] * h, (mix(f.y, t.y) - y0) * unit + n[1] * h], at = [(s.x - x0) * unit, (s.y - y0) * unit], u = [side * s.ax * unit, side * s.ay * unit];
        const { parts, edge: core } = sideOf(run.line, side, s.v, s.h);
        const hold = heldAt(s.h, core - s.h, sideOf(other.line, across, t.v, h).edge - h, (at[0] - edge[0]) * n[0] + (at[1] - edge[1]) * n[1], u[0] * n[0] + u[1] * n[1]);
        return hold >= (parts.at(-1)?.a ?? s.h) ? Infinity : hold;
    };
    /** The held stations from the meet out. */
    const holds = () => {
        const out: { j: number; hold: number }[] = [];
        for (let j = stationIndex(run.stations, end); j >= 0 && j < run.stations.length; j += step) {
            const hold = holdAt(run.stations[j]);
            if (hold === undefined || hold === Infinity) break;
            out.push({ j, hold });
        }
        return out;
    };
    const breaks = () => {
        const out: number[] = [], { stations } = run;
        for (let j = stationIndex(stations, end); j >= 0 && j + step >= 0 && j + step < stations.length && holdAt(stations[j]) !== undefined && holdAt(stations[j]) !== Infinity; j += step) {
            const [s, t] = [stations[j], stations[j + step]], n = Math.ceil(Math.abs(t.along - s.along) / JOIN_STEP_M), solid = sideOf(run.line, side, t.v, t.h).parts.filter(({ flags }) => !(flags & (CF.SAMPLE | CF.MESH))).map(({ a }) => a);
            let last = holdAt(s)!;
            for (let i = 1; i <= n; i++) {
                const at = between(s, t, i / n), hold = holdAt(at) ?? Infinity;
                if (solid.some(point => (last - point) * (hold - point) < 0) || (hold === Infinity && last !== Infinity)) out.push(at.along);
                last = hold;
            }
        }
        return out;
    };
    return { way, which: side > 0 ? 0 : 1, holds, breaks };
}
