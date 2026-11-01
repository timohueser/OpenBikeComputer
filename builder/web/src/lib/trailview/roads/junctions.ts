// The plan of a tile's junctions, a pure function of the tile's lines. The through pair of a junction runs on and
// each other way is a side arm. Between neighbouring ways a corner turns the kerb round an arc, and a way's side
// toward a corner stops at the arc's end. A side arm's carriageway starts in a mouth on the through road's edge, and
// a path or a driveway stops at the road's outer edge. The plan adds to each line the stations that the junctions
// need, and marks what the sweep leaves out to the junction's faces (junction-faces.ts). A road bridge's end is a way of
// its junction: it stays through, and a side arm's mouth runs from its deck's edge.
import { PROFILES, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { inTown, junctionStation, meetingsOf, sideAt, type Arm, type Meeting, type Way } from './arms';
import { cornerOf, type Corner } from './corner';
import { isDeck, markDeck } from './decks';
import { jointsOf, mitre } from './joints';
import { giveWay, lowerAt, zebras, type Paint, type RoadMarks } from './marks';
import { endsOf, type PathEnd } from './path-ends';
import { settleHeights } from './planes';
import { junctionFace, LOOK } from './section';
import { between, CUT, SAME_M, stationIndex } from './stations';
import type { Run } from './sweep';

/** The widest turn between two ways that makes a corner where neither is a side arm, by the fit's tangents, and where one is, by the ways' direction. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** Classes that cross a town road's sidewalk as a driveway. */
const DRIVES = ['service', 'track'];
/** A mouth needs this many metres of its way past it. */
const MOUTH_ROOM_M = 1;

/** A side arm's mouth: the way, metres along it where its whole carriageway starts, its corners by the side of its line, the through ways, and the metres along them that it spans from the junction. */
export interface Mouth { way: Way; far: number; left: Corner; right: Corner; through: Way[]; reach: number }

export interface Junctions { corners: Corner[]; mouths: Mouth[]; ends: PathEnd[] }

/** A lowered kerb ramps back up over this many metres past its stretch. Two paths out of a point, at this cosine or less, are one that runs on. */
const RAMP_M = 1.2, RUNS_ON_COS = -0.5;

/** What a junction does to a line: faces left out, stations it needs, its paint and its lowered kerbs. */
type Marks = { cuts: { from: number; to: number; bits: number }[]; breaks: number[]; paints: { from: number; to: number; paint: Paint }[]; drops: { from: number; to: number; side: 1 | -1 }[] };
/** Marks a way from metres `from` to `to` with bits of CUT, and asks for a station at `to`. */
export type Mark = (way: { run: Run }, from: number, to: number, bits: number) => void;

const CLASSES = Object.keys(PROFILES);

/** The road arms with a corridor. */
const groundOf = (arms: Arm[]) => arms.filter((arm): arm is Way => !!arm.run);

/** The cosine of the angle between two ways out of their junction, by the fit's tangents there: a bend's chords hide them. */
function tangentCos(p: Way, q: Way): number {
    const s = junctionStation(p), t = junctionStation(q);
    return (p.step * q.step * (s.ax * t.ax + s.ay * t.ay)) / Math.hypot(s.ax, s.ay) / Math.hypot(t.ax, t.ay);
}

/** Whether way p has a kerb on its side toward way w. */
const kerbedToward = (p: Way, w: Arm) => sideAt(p, (w.u[0] * p.u[1] - w.u[1] * p.u[0] > 0 ? p.step : -p.step) as 1 | -1).walk > 0;

/** The side arms that cross a town road's sidewalk: they stop at its outer edge. */
function drivesOf(arms: Arm[], pair: Arm[]): Way[] {
    // A deck's walk ends at its parapet's cap: what leaves there has a mouth.
    if (!pair.every(({ run }) => run) || pair.some(({ line }) => isDeck(line))) return [];
    return arms.filter((arm): arm is Way => !arm.through && !!arm.run && DRIVES.includes(CLASSES[arm.line.profile]) && !inTown(arm as Way) && pair.some(p => kerbedToward(p as Way, arm)));
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

/** Plans a junction of three road ways or more; gives back its corners, and the arms that end at its roads' edges instead. */
function junctionAt(lines: Line[], meeting: Meeting, out: Junctions, mark: Mark, marks: RoadMarks): { drives: Way[]; corners: Corner[] } {
    const arms = meeting.roads, pair = arms.filter(({ through }) => through);
    if (arms.length < 3 || pair.length !== 2 || groundOf(arms).length < 2) return { drives: [], corners: [] };
    const drives = drivesOf(arms, pair), faced = pair.map(({ line }) => line).filter(line => junctionFace(line)), road = faced.find(line => junctionFace(line)!.mat === LOOK.ASPHALT) ?? faced[0];
    // A pair of soil has no surface to join: the ruts cross, and the ground shows between.
    if (!road) return { drives, corners: [] };
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
    return { drives, corners };
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
        const share = (side: 1 | -1, bit: number) => (((s.cut ?? 0) | (stations[j - 1]?.cut ?? 0)) & bit ? 0 : Math.max(0, ...drops.filter(drop => drop.side === side).map(({ from, to }) => {
            const past = Math.max(from - s.along, s.along - to, 0) / RAMP_M;
            return past < 1 ? 1 - past * past * (3 - 2 * past) : 0;
        })));
        s.drop = [share(1, CUT.LEFT), share(-1, CUT.RIGHT)];
    });
    run.stations = stations;
}

/**
 * Plans the junctions of a tile's lines. Each line's stations take what the plan needs: every tile that holds a
 * junction within REACH_M reads the same WINDOW_M of its ways, so they all give it the same corners and cuts. Heights
 * chain further: a mouth past a tile's buffer can raise a through way that a nearer junction then reads.
 */
export function planJunctions(lines: Line[], runs: Run[], crossings: { x: number; y: number }[] = []): Junctions {
    const out: Junctions = { corners: [], mouths: [], ends: [] }, marks = new Map<Run, Marks>();
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
    for (const meeting of meetingsOf(lines, runs)) {
        const { drives, corners } = junctionAt(lines, meeting, out, mark, roadMarks), ground = groundOf(meeting.roads).filter(arm => !drives.includes(arm));
        if (ground.length < 2) continue;
        const ends = endsOf(ground, [...meeting.paths, ...drives], corners, mark);
        // A driveway crosses the sidewalk, and a path that runs on crosses the road. A path that stops on the sidewalk leaves its kerb whole.
        for (const { way, walk } of ends) if (walk && (drives.includes(way) || ends.some(other => other.way.u[0] * way.u[0] + other.way.u[1] * way.u[1] < RUNS_ON_COS))) lowerAt(way, ground, roadMarks);
        out.ends.push(...ends);
    }
    zebras(runs, crossings, roadMarks);
    for (const run of runs) if (isDeck(run.line)) markDeck(run, lines, mark);
    for (const [run, entry] of marks) settle(run, entry);
    const joints = jointsOf(lines, runs);
    mitre(joints);
    settleHeights(out.mouths, out.ends, joints);
    return out;
}
