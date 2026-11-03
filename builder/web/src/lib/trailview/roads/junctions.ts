// The faces of a tile's junctions, from its lines and the junction plan of terrain/junction-plan.ts, which says what
// meets where. Between neighbouring ways a corner turns the kerb round an arc, and a way's side toward a corner stops at
// the arc's end. A side road's carriageway starts at its stop, in a mouth on the through road's edge, and a path or a
// driveway stops at its seam. This adds to each line the stations that the junctions need, and marks what the sweep
// leaves out to the junction's faces (junction-faces.ts), and where a sidewalk ends. A road bridge's end is a way of
// its junction: it stays through, and a side road's mouth runs from its deck's edge.
import { GUEST_INSIDE, WALK_END_M, type JunctionPlan, type Arm as PlanArm } from '../terrain/junction-plan';
import { PAVED_CODES, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { meetingsOf, stationAlong, vertexAlong, type Arm, type Way } from './arms';
import { cornerOf, type Corner } from './corner';
import { isDeck, markDeck } from './decks';
import { jointsOf, mitre } from './joints';
import { giveWay, lowerAt, zebras, type Paint, type RoadMarks } from './marks';
import { junctionFace, LOOK } from './section';
import { between, CUT, SAME_M, stationIndex } from './stations';
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

/** Where a sidewalk ends on a side of a line: the metres along its stations, whether the sidewalk lies ahead of there (1) or behind (-1), and the metres of its taper. */
type End = { at: number; toward: 1 | -1; side: 1 | -1; length: number };
/** What a junction does to a line: faces left out, stations it needs, its paint, its lowered kerbs and its sidewalks' ends. */
type Marks = { cuts: { from: number; to: number; bits: number }[]; breaks: number[]; paints: { from: number; to: number; paint: Paint }[]; drops: { from: number; to: number; side: 1 | -1 }[]; ends: End[] };
/** Marks a way from metres `from` to `to` with bits of CUT, and asks for a station at `to`. */
export type Mark = (way: { run: Run }, from: number, to: number, bits: number) => void;

/** The road arms with a corridor. */
const groundOf = (arms: Arm[]) => arms.filter((arm): arm is Way => !!arm.run);

/** Adds a line's stations and sets its cuts, its paint, its lowered kerbs and the share of each sidewalk's end. */
function settle(run: Run, { cuts, breaks, paints, drops, ends }: Marks): void {
    const asked = [...breaks, ...paints.flatMap(({ from, to }) => [from, to]), ...drops.flatMap(({ from, to }) => [from - RAMP_M, from, to, to + RAMP_M]), ...ends.map(({ at, toward, length }) => at + toward * length)];
    const stations = run.stations.slice(), wanted = [...new Set(asked)].sort((p, q) => p - q);
    for (const along of wanted) {
        const j = stationIndex(stations, along), near = stations[j];
        if (Math.abs(near.along - along) < BREAK_SNAP_M) continue;
        const k = near.along < along ? j : j - 1;
        if (k < 0 || k + 1 >= stations.length) continue;
        stations.splice(k + 1, 0, between(stations[k], stations[k + 1], (along - stations[k].along) / (stations[k + 1].along - stations[k].along)));
    }
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
    run.stations = stations;
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
    const marksOf = (run: Run) => marks.get(run) ?? marks.set(run, { cuts: [], breaks: [], paints: [], drops: [], ends: [] }).get(run)!;
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
        const wayOf = ({ line, step }: Pick<PlanArm, 'line' | 'step'>) => all.find(arm => arm.line === line && arm.step === step);
        const pair = meeting.roads.filter(({ through }) => through), faced = pair.map(({ line }) => line).filter(line => junctionFace(line));
        const road = faced.find(line => junctionFace(line)!.mat === LOOK.ASPHALT) ?? faced[0];
        const corners = plan.corners.filter(here).flatMap(corner => {
            const p = wayOf(corner.p), q = wayOf(corner.q);
            return p && q && road ? [cornerOf(corner, meeting, p, q, road)] : [];
        });
        for (const corner of corners) ([[corner.p, corner.sp, 0], [corner.q, corner.sq, 1]] as const).forEach(([way, side, end]) => {
            mark(way, way.along, corner.ends[end], side > 0 ? CUT.LEFT : CUT.RIGHT);
            if (way.through) mark(way, corner.meet[end], corner.meet[end], 0);
        });
        out.corners.push(...corners);
        const guests = plan.guests.filter(here);
        for (const guest of guests) {
            const way = wayOf(guest);
            if (guest.kind !== 'side' || !way) continue;
            const far = stationAlong(way.run.stations, way.line, guest.stop), before = corners.find(({ q }) => q === way), after = corners.find(({ p }) => p === way);
            if (!before || !after) continue;
            way.mouth = far;
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
            const way = guest.kind === 'side' ? undefined : wayOf(guest);
            return way ? [{ way, at: stationAlong(way.run.stations, way.line, guest.stop), hosts, walk: guest.walk, drive: guest.kind === 'driveway' }] : [];
        });
        // A driveway crosses the sidewalk, and a path that runs on crosses the road. A path that stops on the sidewalk leaves its kerb whole.
        for (const { way, walk, drive } of seams) if (walk && (drive || seams.some(other => other.way.u[0] * way.u[0] + other.way.u[1] * way.u[1] < RUNS_ON_COS))) lowerAt(way, ways, roadMarks);
        out.seams.push(...seams.map(({ way, at, hosts: on, walk }) => ({ way, at, hosts: on, walk })));
    }
    zebras(runs, crossings, roadMarks);
    for (const run of runs) if (isDeck(run.line)) markDeck(run, lines, mark);
    // Where a sidewalk ends, its kerb turns to its back edge and its slab narrows to nothing over WALK_END_M, or over half
    // a shorter piece, and short of a junction's cut, so a corner's ring meets a whole kerb. The faces from station j to
    // j + 1 take the sides of segment v of station j + 1, so a change lies at a station from 1 on.
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
    for (const [run, entry] of marks) settle(run, entry);
    mitre(jointsOf(lines, runs));
    return out;
}
