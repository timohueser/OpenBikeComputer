// Where OSM splits a road, its two end stations become one section: the bisector of the fit's tangents, so no wedge
// opens between the ways where they turn, and the mean of their banks, as two fitted strokes can end on two banks
// and their edges would stand apart.
import { partnersOf } from '../terrain/corners';
import type { Line } from '../terrain/bins';
import type { Run } from './sweep';

/** Two tangents that cancel out this far have no bisector; a mitre stretches a section by at most the inverse of the cosine here. */
const LEAST_SUM = 0.3, LEAST_COS = 0.5;

export function mitre(lines: Line[], runs: Run[]): void {
    const partners = partnersOf(lines), runOf = new Map(runs.map(run => [run.line, run] as const)), moved: (() => void)[] = [];
    for (const run of runs) {
        partners.get(run.line)?.forEach((other, end) => {
            const them = other && runOf.get(other);
            if (!them) return;
            const s = run.stations[end ? run.stations.length - 1 : 0], first = them.stations[0], same = first.x === s.x && first.y === s.y, o = same ? first : them.stations[them.stations.length - 1];
            if (o.x !== s.x || o.y !== s.y) return;
            // Across is a tangent turned to its left, per metre: both in this line's sense.
            const flip = (end === 1) === same ? 1 : -1, own = Math.hypot(s.ax, s.ay), theirs = Math.hypot(o.ax, o.ay), bank = (s.bank + flip * o.bank) / 2;
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
