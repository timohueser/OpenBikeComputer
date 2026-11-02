// The faces of a tile's junctions, from its lines and the junction plan of terrain/junction-plan.ts. The through pair
// of a junction runs on and each other way is a side arm. Between neighbouring ways a corner turns the kerb round an
// arc, and a way's side toward a corner stops at the arc's end. A side arm's carriageway starts in a mouth on the
// through road's edge, and a guest of the plan stops at its seam. This adds to each line the stations that the
// junctions need, and marks what the sweep leaves out to the junction's faces (junction-faces.ts). A road bridge's end
// is a way of its junction: it stays through, and a side arm's mouth runs from its deck's edge.
import type { Guest } from '../terrain/junction-plan';
import { PAVED_CODES, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { junctionStation, meetingsOf, stationAlong, vertexAlong, type Arm, type Meeting, type Way } from './arms';
import { cornerOf, type Corner } from './corner';
import { isDeck, markDeck } from './decks';
import { jointsOf, mitre } from './joints';
import { giveWay, lowerAt, zebras, type Paint, type RoadMarks } from './marks';
import { settleHeights } from './planes';
import { junctionFace, LOOK } from './section';
import { between, CUT, SAME_M, stationIndex } from './stations';
import type { Run } from './sweep';

/** The widest turn between two ways that makes a corner where neither is a side arm, by the fit's tangents, and where one is, by the ways' direction. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** A mouth needs this many metres of its way past it. */
const MOUTH_ROOM_M = 1;

/** A side arm's mouth: the way, metres along it where its whole carriageway starts, its corners by the side of its line, the through ways, and the metres along them that it spans from the junction. */
export interface Mouth { way: Way; far: number; left: Corner; right: Corner; through: Way[]; reach: number }

/** A guest's stop on its hosts: the way, metres along it at the stop, the through ways, and whether its seam is a sidewalk's outer edge. */
export interface Seam { way: Way; at: number; hosts: Way[]; walk: boolean }

/** A junction's faces, its guests' seams, and the roads that end where only paths run on. */
export interface Junctions { corners: Corner[]; mouths: Mouth[]; seams: Seam[]; plain: Way[] }

/** A lowered kerb ramps back up over this many metres past its stretch. Two paths out of a point, at this cosine or less, are one that runs on. */
const RAMP_M = 1.2, RUNS_ON_COS = -0.5;

/** What a junction does to a line: faces left out, stations it needs, its paint and its lowered kerbs. */
type Marks = { cuts: { from: number; to: number; bits: number }[]; breaks: number[]; paints: { from: number; to: number; paint: Paint }[]; drops: { from: number; to: number; side: 1 | -1; end?: boolean }[] };
/** Marks a way from metres `from` to `to` with bits of CUT, and asks for a station at `to`. */
export type Mark = (way: { run: Run }, from: number, to: number, bits: number) => void;

/** The road arms with a corridor. */
const groundOf = (arms: Arm[]) => arms.filter((arm): arm is Way => !!arm.run);

/** The cosine of the angle between two ways out of their junction, by the fit's tangents there: a bend's chords hide them. */
function tangentCos(p: Way, q: Way): number {
    const s = junctionStation(p), t = junctionStation(q);
    return (p.step * q.step * (s.ax * t.ax + s.ay * t.ay)) / Math.hypot(s.ax, s.ay) / Math.hypot(t.ax, t.ay);
}

/** The corners between neighbouring ways: none between two side arms, and none where the through pair runs on in about one curve. */
function cornersAt(lines: Line[], meeting: Meeting, ways: Arm[]): Corner[] {
    return ways.flatMap((p, i) => {
        const q = ways[(i + 1) % ways.length], turn = q.angle - p.angle + (i + 1 === ways.length ? 2 * Math.PI : 0);
        if ((!p.through && !q.through) || !p.run || !q.run) return [];
        if (p.through && q.through ? tangentCos(p as Way, q as Way) <= Math.cos(MAX_TURN) : turn >= MAX_SIDE_TURN) return [];
        return cornerOf(lines, meeting, p as Way, q as Way) ?? [];
    });
}

/** The mouths of the side arms that have a corner on both sides and room past them. */
function mouthsAt(ways: Arm[], corners: Corner[], pair: Arm[]): Mouth[] {
    return ways.flatMap(arm => {
        const before = corners.find(({ q }) => q === arm), after = corners.find(({ p }) => p === arm);
        // A deck that leaves as a side starts at the road's edge.
        if (arm.through || isDeck(arm.line) || !before || !after) return [];
        const way = arm as Way, { stations } = way.run, ends = [before.meet[1], after.meet[0]], far = way.step > 0 ? Math.max(...ends) : Math.min(...ends), past = far + way.step * MOUTH_ROOM_M;
        if (past < 0 || past > stations[stations.length - 1].along) return [];
        way.mouth = far;
        const reach = Math.max(...[before, after].map(({ p, q, meet }) => Math.abs(p.through ? meet[0] - p.along : meet[1] - q.along)));
        return { way, far, left: before.sq > 0 ? before : after, right: before.sq > 0 ? after : before, through: pair.filter((t): t is Way => !!t.run), reach };
    });
}

/** Plans a junction of three road ways or more but its drives, which stop at their seams. */
function junctionAt(lines: Line[], meeting: Meeting, out: Junctions, mark: Mark, marks: RoadMarks, drives: Way[]): void {
    const arms = meeting.roads, pair = arms.filter(({ through }) => through);
    if (arms.length < 3 || pair.length !== 2 || groundOf(arms).length < 2) return;
    const faced = pair.map(({ line }) => line).filter(line => junctionFace(line)), road = faced.find(line => junctionFace(line)!.mat === LOOK.ASPHALT) ?? faced[0];
    // A pair of soil has no surface to join: the ruts cross, and the ground shows between.
    if (!road) return;
    const ways = arms.filter(arm => !(drives as Arm[]).includes(arm)), found = cornersAt(lines, meeting, ways), mouths = mouthsAt(ways, found, pair);
    // A corner of a side arm with no mouth has no surface to meet.
    const corners = found.filter(({ p, q }) => (p.through || p.mouth !== undefined) && (q.through || q.mouth !== undefined));
    for (const corner of corners) {
        corner.road = road;
        ([[corner.p, corner.sp, 0], [corner.q, corner.sq, 1]] as const).forEach(([way, side, end]) => {
            mark(way, way.along, corner.ends[end], side > 0 ? CUT.LEFT : CUT.RIGHT);
            if (way.through) mark(way, corner.meet[end], corner.meet[end], 0);
        });
    }
    for (const { way, far, through } of mouths) {
        mark(way, way.along, far, CUT.ROAD);
        giveWay(way, far, through, marks);
    }
    out.corners.push(...corners);
    out.mouths.push(...mouths);
}

/** Adds a line's stations and sets its cuts. */
function settle(run: Run, { cuts, breaks, paints, drops }: Marks): void {
    const asked = [...breaks, ...paints.flatMap(({ from, to }) => [from, to]), ...drops.flatMap(({ from, to }) => [from - RAMP_M, from, to, to + RAMP_M])];
    const stations = run.stations.slice(), wanted = [...new Set(asked)].sort((p, q) => p - q);
    for (const along of wanted) {
        const j = stationIndex(stations, along), near = stations[j];
        if (Math.abs(near.along - along) < SAME_M) continue;
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
    if (drops.length) stations.forEach((s, j) => {
        // A corner's ring stands on the full kerb of the station where the side's sweep stops.
        const share = (side: 1 | -1, bit: number, end: boolean) => (((s.cut ?? 0) | (stations[j - 1]?.cut ?? 0)) & bit ? 0 : Math.max(0, ...drops.filter(drop => drop.side === side && !!drop.end === end).map(({ from, to }) => {
            const past = Math.max(from - s.along, s.along - to, 0) / RAMP_M;
            return past < 1 ? 1 - past * past * (3 - 2 * past) : 0;
        })));
        s.drop = [share(1, CUT.LEFT, false), share(-1, CUT.RIGHT, false)];
        const end: [number, number] = [share(1, CUT.LEFT, true), share(-1, CUT.RIGHT, true)];
        if (end[0] || end[1]) s.end = end;
    });
    run.stations = stations;
}

/** The metres along a run's stations of each stretch of its line's segments that lie inside a host. */
function insideOf({ line, stations }: Run): [number, number][] {
    const out: [number, number][] = [], inside = line.inside ?? [];
    for (let s = 0; s < inside.length; s++) {
        if (!inside[s]) continue;
        let e = s;
        while (e + 1 < inside.length && inside[e + 1]) e++;
        out.push([vertexAlong(stations, s), vertexAlong(stations, e + 1)]);
        s = e;
    }
    return out;
}

/**
 * Plans the junctions of a tile's lines. Each line's stations take what the plan needs: every tile that holds a
 * junction within REACH_M reads the same WINDOW_M of its ways, so they all give it the same corners and cuts. Heights
 * chain further: a mouth past a tile's buffer can raise a through way that a nearer junction then reads.
 */
export function planJunctions(lines: Line[], runs: Run[], guests: Guest[], crossings: { x: number; y: number }[] = []): Junctions {
    const out: Junctions = { corners: [], mouths: [], seams: [], plain: [] }, marks = new Map<Run, Marks>();
    const marksOf = (run: Run) => marks.get(run) ?? marks.set(run, { cuts: [], breaks: [], paints: [], drops: [] }).get(run)!;
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
    // What a host holds draws nothing of its guest.
    for (const run of runs) for (const [from, to] of insideOf(run)) mark({ run }, from, to, CUT.ALL);
    for (const meeting of meetingsOf(lines, runs)) {
        const guestOf = (arm: Arm) => guests.find(g => !g.mouth && g.line === arm.line && g.step === arm.step && g.x === meeting.x && g.y === meeting.y);
        const drives = groundOf(meeting.roads).filter(arm => guestOf(arm));
        junctionAt(lines, meeting, out, mark, roadMarks, drives);
        const ground = groundOf(meeting.roads).filter(arm => !drives.includes(arm)), hosts = ground.filter(({ through }) => through);
        // A road's end where only paths run on has a plain end.
        if (meeting.roads.length === 1 && ground.length === 1 && !!meeting.paths.length && PAVED_CODES.includes(ground[0].line.code) && !isDeck(ground[0].line)) out.plain.push(ground[0]);
        if (!hosts.length) continue;
        const seams = [...meeting.paths, ...drives].flatMap(way => {
            const guest = guestOf(way);
            return guest ? [{ way, at: stationAlong(way.run.stations, way.line, guest.stop), hosts, walk: guest.walk }] : [];
        });
        // A driveway crosses the sidewalk, and a path that runs on crosses the road. A path that stops on the sidewalk leaves its kerb whole.
        for (const { way, walk } of seams) if (walk && (drives.includes(way) || seams.some(other => other.way.u[0] * way.u[0] + other.way.u[1] * way.u[1] < RUNS_ON_COS))) lowerAt(way, ground, roadMarks);
        out.seams.push(...seams);
    }
    zebras(runs, crossings, roadMarks);
    for (const run of runs) if (isDeck(run.line)) markDeck(run, lines, mark);
    // Where a sidewalk ends, its kerb and its slab ramp down to the road, so its end stands open nowhere. The faces from
    // station j to j + 1 take the sides of segment v of station j + 1, so a change lies at a station from 1 on.
    for (const run of runs) for (const side of [1, -1] as const) {
        const walked = (j: number) => ((run.line.sidewalks?.[run.stations[j].v] ?? 0) & (side > 0 ? LEFT : RIGHT)) !== 0;
        for (let j = 1; j + 1 < run.stations.length; j++) if (walked(j) !== walked(j + 1)) marksOf(run).drops.push({ from: run.stations[j].along, to: run.stations[j].along, side, end: true });
    }
    for (const [run, entry] of marks) settle(run, entry);
    const joints = jointsOf(lines, runs);
    mitre(joints);
    settleHeights(out.mouths, joints);
    return out;
}
