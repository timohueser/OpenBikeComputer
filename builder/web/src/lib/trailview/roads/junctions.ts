// The plan of a tile's junctions, a pure function of the tile's lines. The through pair of a junction runs on and
// each other way is a side arm. Between neighbouring ways a corner turns the kerb round an arc, and a way's side
// toward a corner stops at the arc's end. A side arm's carriageway starts in a mouth on the through road's edge, and
// a path or a driveway stops at the road's outer edge. The plan adds to each line the stations that the junctions
// need, and marks what the sweep leaves out to the junction's faces (junction-faces.ts).
import { PROFILES, ROAD, type Line } from '../terrain/bins';
import { inTown, meetingsOf, type Arm, type Meeting, type Way } from './arms';
import { cornerOf, type Corner } from './corner';
import { mitre } from './mitre';
import { endsOf, type PathEnd } from './path-ends';
import { settleHeights } from './planes';
import { junctionFace, MAT, sideOf } from './section';
import { between, CUT, stationAt, type Station } from './stations';
import type { Run } from './sweep';

/** The widest turn between two ways that makes a corner where neither is a side arm, and where one is. */
const MAX_TURN = (150 * Math.PI) / 180, MAX_SIDE_TURN = (172 * Math.PI) / 180;
/** Classes that cross a town road's sidewalk as a driveway. */
const DRIVES = ['service', 'track'];
/** A pair opens at least this far: the cosine of the bake's limit. A pair this straight runs on as one curve. */
const THROUGH_COS = -0.3, RUNS_ON_COS = -0.97;
/** A mouth needs this many metres of its way past it. */
const MOUTH_ROOM_M = 1;
/** Two stations this near are one. */
const SAME_M = 0.01;

/** A side arm's mouth: the way, metres along it where its whole carriageway starts, its corners by the side of its line, the through ways, and the metres along them that it spans from the junction. */
export interface Mouth { way: Way; far: number; left: Corner; right: Corner; through: Way[]; reach: number }

export interface Junctions { corners: Corner[]; mouths: Mouth[]; ends: PathEnd[] }

/** What a junction does to a line: faces left out, and stations it needs. */
type Marks = { cuts: { from: number; to: number; bits: number }[]; breaks: number[] };
/** Marks a way from metres `from` to `to` with bits of CUT, and asks for a station at `to`. */
export type Mark = (way: Way, from: number, to: number, bits: number) => void;

const CLASSES = Object.keys(PROFILES);

/** The arms on the ground with a corridor. */
const groundOf = (arms: Arm[]) => arms.filter((arm): arm is Way => arm.line.kind === ROAD && !!arm.run);

/** The station of a way at its junction. */
const stationOf = ({ run: { stations }, along }: Way) => stations[stationAt(stations, along)];

/** The cosine of the angle between two ways out of their junction, by the fit's tangents there: a bend's chords hide them. */
function tangentCos(p: Way, q: Way): number {
    const s = stationOf(p), t = stationOf(q);
    return (p.step * q.step * (s.ax * t.ax + s.ay * t.ay)) / Math.hypot(s.ax, s.ay) / Math.hypot(t.ax, t.ay);
}

/**
 * The through pair of a junction: the bake's, but two ways of the best class that leave opposite stay it. At a
 * hairpin's apex their chords open too little to pair, and the bake pairs one of them with a track there.
 */
function pairOf(arms: Arm[]): Arm[] {
    const best = Math.min(...arms.map(({ line }) => line.profile)), first = arms.filter(({ line }) => line.profile === best);
    return first.length === 2 && first.every(({ run }) => run) && tangentCos(first[0] as Way, first[1] as Way) <= THROUGH_COS ? first : arms.filter(({ through }) => through);
}

/** Whether way p has a kerb on its side toward way w. */
function kerbedToward(p: Way, w: Arm): boolean {
    const s = stationOf(p), side = (w.u[0] * p.u[1] - w.u[1] * p.u[0] > 0 ? p.step : -p.step) as 1 | -1;
    return sideOf(p.line, side, s.v, s.h).parts.some(({ mat }) => mat === MAT.KERB);
}

/** The side arms that cross a town road's sidewalk: they stop at its outer edge. */
function drivesOf(arms: Arm[], pair: Arm[]): Way[] {
    if (!pair.every(({ run }) => run)) return [];
    return arms.filter((arm): arm is Way => !arm.through && !!arm.run && DRIVES.includes(CLASSES[arm.line.profile]) && !inTown(arm as Way) && pair.some(p => kerbedToward(p as Way, arm)));
}

/** The corners between neighbouring ways: none between two side arms, and none where the through pair runs on as one curve. */
function cornersAt(lines: Line[], meeting: Meeting, ways: Arm[]): Corner[] {
    return ways.flatMap((p, i) => {
        const q = ways[(i + 1) % ways.length], turn = q.angle - p.angle + (i + 1 === ways.length ? 2 * Math.PI : 0), pair = p.through && q.through;
        if ((!p.through && !q.through) || !p.run || !q.run || turn >= (pair ? MAX_TURN : MAX_SIDE_TURN) || (pair && tangentCos(p as Way, q as Way) <= RUNS_ON_COS)) return [];
        return cornerOf(lines, meeting, p as Way, q as Way) ?? [];
    });
}

/** The mouths of the side arms that have a corner on both sides and room past them. */
function mouthsAt(ways: Arm[], corners: Corner[], pair: Arm[]): Mouth[] {
    return ways.flatMap(arm => {
        const before = corners.find(({ q }) => q === arm), after = corners.find(({ p }) => p === arm);
        if (arm.through || !before || !after) return [];
        const way = arm as Way, { stations } = way.run, ends = [before.meet[1], after.meet[0]], far = way.step > 0 ? Math.max(...ends) : Math.min(...ends), past = far + way.step * MOUTH_ROOM_M;
        if (past < 0 || past > stations[stations.length - 1].along) return [];
        way.mouth = far;
        const reach = Math.max(...[before, after].map(({ p, q, meet }) => Math.abs(p.through ? meet[0] - p.along : meet[1] - q.along)));
        return { way, far, left: before.sq > 0 ? before : after, right: before.sq > 0 ? after : before, through: pair.filter((t): t is Way => !!t.run), reach };
    });
}

/** Plans a junction of three road ways or more; gives back its corners, and the arms that end at its roads' edges instead. */
function junctionAt(lines: Line[], meeting: Meeting, out: Junctions, mark: Mark): { drives: Way[]; corners: Corner[] } {
    const arms = meeting.roads, pair = pairOf(arms);
    if (arms.length < 3 || pair.length !== 2 || groundOf(arms).length < 2) return { drives: [], corners: [] };
    for (const arm of arms) arm.through = pair.includes(arm);
    const drives = drivesOf(arms, pair);
    // Ways of soil have no surface to join: their ruts cross, and the ground shows between.
    if (!pair.every(({ line }) => junctionFace(line))) return { drives, corners: [] };
    const ways = arms.filter(arm => !(drives as Arm[]).includes(arm)), found = cornersAt(lines, meeting, ways), mouths = mouthsAt(ways, found, pair);
    // A corner of a side arm with no mouth has no surface to meet.
    const corners = found.filter(({ p, q }) => (p.through || p.mouth !== undefined) && (q.through || q.mouth !== undefined));
    for (const corner of corners) {
        ([[corner.p, corner.sp, 0], [corner.q, corner.sq, 1]] as const).forEach(([way, side, end]) => {
            mark(way, way.along, corner.ends[end], side > 0 ? CUT.LEFT : CUT.RIGHT);
            if (way.through) mark(way, corner.meet[end], corner.meet[end], 0);
        });
    }
    for (const { way, far } of mouths) mark(way, way.along, far, CUT.ROAD);
    out.corners.push(...corners);
    out.mouths.push(...mouths);
    return { drives, corners };
}

/** Adds a line's stations and sets its cuts. */
function settle(run: Run, { cuts, breaks }: Marks): void {
    const stations = run.stations.slice(), wanted = [...new Set(breaks)].sort((p, q) => p - q);
    for (const along of wanted) {
        const j = stationAt(stations, along), near = stations[j];
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
    }
    run.stations = stations;
}

/**
 * Plans the junctions of a tile's lines. Each line's stations take what the plan needs: every tile that holds a
 * junction within REACH_M reads the same WINDOW_M of its ways, so they all change a station alike.
 */
export function planJunctions(lines: Line[], runs: Run[]): Junctions {
    const out: Junctions = { corners: [], mouths: [], ends: [] }, marks = new Map<Run, Marks>();
    const mark = ({ run }: Way, from: number, to: number, bits: number) => {
        const entry = marks.get(run) ?? marks.set(run, { cuts: [], breaks: [] }).get(run)!;
        entry.breaks.push(to);
        if (bits) entry.cuts.push({ from, to, bits });
    };
    for (const meeting of meetingsOf(lines, runs)) {
        const { drives, corners } = junctionAt(lines, meeting, out, mark), ground = groundOf(meeting.roads).filter(arm => !drives.includes(arm));
        if (ground.length >= 2) out.ends.push(...endsOf(ground, [...meeting.paths, ...drives], corners, mark));
    }
    for (const [run, entry] of marks) settle(run, entry);
    mitre(lines, runs);
    settleHeights(out.mouths, out.ends);
    return out;
}

/** The station of a line at metres `along`, which the plan put there. */
export function stationOn({ stations }: Run, along: number): Station {
    return stations[stationAt(stations, along)];
}
