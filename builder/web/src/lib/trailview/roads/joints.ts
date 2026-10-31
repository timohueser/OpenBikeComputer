// Where OSM splits a road, its two end stations are one section: the bisector of the fit's tangents, so no wedge
// opens between the ways where they turn, and the mean of their banks, as two fitted strokes can end on two banks.
// What a junction does to a way's heights runs on across such a joint.
import { partnersOf } from '../terrain/corners';
import type { Line } from '../terrain/bins';
import { stationIndex, type Station } from './stations';
import type { Run } from './sweep';

/** Two tangents that cancel out this far have no bisector; a mitre stretches a section by at most the inverse of the cosine here. */
const LEAST_SUM = 0.3, LEAST_COS = 0.5;

/** The run that an end of a line runs on into: which end of it meets, and -1 where the two lines run head to head. */
export interface Joint { run: Run; end: 0 | 1; flip: 1 | -1 }

/** Per run, the joints at its start and at its end. */
export type Joints = Map<Run, (Joint | undefined)[]>;

const endOf = ({ stations }: Run, end: 0 | 1) => stations[end ? stations.length - 1 : 0];

export function jointsOf(lines: Line[], runs: Run[]): Joints {
    const partners = partnersOf(lines), runOf = new Map(runs.map(run => [run.line, run] as const));
    return new Map(runs.map(run => [run, ([0, 1] as const).map(end => {
        const other = partners.get(run.line)?.[end], them = other && runOf.get(other);
        if (!them) return undefined;
        const s = endOf(run, end), meets = [0, 1].find(at => endOf(them, at as 0 | 1).x === s.x && endOf(them, at as 0 | 1).y === s.y) as 0 | 1 | undefined;
        return meets === undefined ? undefined : { run: them, end: meets, flip: (end === 1) === (meets === 0) ? 1 : -1 };
    })] as const));
}

/** Makes the two end stations of each joint one section. */
export function mitre(joints: Joints): void {
    const moved: (() => void)[] = [];
    for (const [run, ends] of joints) {
        ends.forEach((joint, end) => {
            if (!joint) return;
            // Across is a tangent turned to its left, per metre: both in this line's sense.
            const s = endOf(run, end as 0 | 1), o = endOf(joint.run, joint.end), { flip } = joint, own = Math.hypot(s.ax, s.ay), theirs = Math.hypot(o.ax, o.ay), bank = (s.bank + flip * o.bank) / 2;
            const ax = s.ax / own + flip * o.ax / theirs, ay = s.ay / own + flip * o.ay / theirs, l = Math.hypot(ax, ay), stretch = own / l / Math.max(LEAST_COS, (ax * s.ax + ay * s.ay) / l / own);
            moved.push(() => {
                s.bank = bank;
                if (l < LEAST_SUM) return;
                s.ax = ax * stretch;
                s.ay = ay * stretch;
            });
        });
    }
    for (const move of moved) move();
}

/** The stations up to `metres` from metres `along` of a run, stepping `step`, on across its joints: each with its metres from there, and -1 where its left is the first run's right. */
export function* onward(joints: Joints, run: Run, along: number, step: 1 | -1, metres: number): Generator<{ s: Station; metres: number; flip: 1 | -1 }> {
    let flip: 1 | -1 = 1, done = 0;
    for (;;) {
        const { stations } = run;
        for (let j = stationIndex(stations, along); j >= 0 && j < stations.length; j += step) {
            const d = done + Math.abs(stations[j].along - along);
            if (d > metres) return;
            yield { s: stations[j], metres: d, flip };
        }
        const joint = joints.get(run)?.[step > 0 ? 1 : 0];
        if (!joint) return;
        done += Math.abs(endOf(run, step > 0 ? 1 : 0).along - along);
        along = endOf(joint.run, joint.end).along;
        step = joint.end ? -1 : 1;
        flip = (flip * joint.flip) as 1 | -1;
        run = joint.run;
    }
}
