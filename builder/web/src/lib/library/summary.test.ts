import { describe, expect, it } from 'vitest';
import type { RouteSummary } from './records';
import { rideSummary, summarize, type Point } from './summary';

const metresPerDegree = 6_371_008.8 * Math.PI / 180;
const numbers = { lengthM: 0, climbM: 0, seconds: 0, days: 1, bike: 'road' } as const;

/** Metres from `p` to the nearest segment of `line`, in a flat frame at the latitude of `p`. */
function offset(p: Point, line: number[]): number {
    const k = Math.cos(p[1] * Math.PI / 180) * metresPerDegree;
    let best = Infinity;
    for (let i = 2; i < line.length; i += 2) {
        const ax = (line[i - 2] - p[0]) * k, ay = (line[i - 1] - p[1]) * metresPerDegree;
        const bx = (line[i] - p[0]) * k - ax, by = (line[i + 1] - p[1]) * metresPerDegree - ay;
        const t = Math.max(0, Math.min(1, -(ax * bx + ay * by) / (bx * bx + by * by || 1)));
        best = Math.min(best, Math.hypot(ax + t * bx, ay + t * by));
    }
    return best;
}

describe('summary', () => {
    it('keeps a bend of 15 m, and simplifies a long winding line to 1,000 points within 40 m', () => {
        const bend = summarize<RouteSummary>([[8, 47, 500], [8.001, 47 + 15 / metresPerDegree, 510.4], [8.002, 47, null]], numbers);
        expect([bend.heights, bend.highestM, bend.bounds[3]]).toEqual([[500, 510, null], 510, 47 + 15 / metresPerDegree]);

        // 100 km to the east, a point every 20 m, with a sideways wave of 25 m.
        const winding: Point[] = Array.from({ length: 5000 }, (_, i) => [8 + i * 20 / (metresPerDegree * Math.cos(47 * Math.PI / 180)),
            47 + 25 * Math.sin(i * 20 * 2 * Math.PI / 400) / metresPerDegree, null]);
        const { line } = summarize<RouteSummary>(winding, numbers);
        expect(line.length / 2).toBeLessThanOrEqual(1000);
        expect(Math.max(...winding.map(p => offset(p, line)))).toBeLessThan(41);
    });

    it('measures a ride with a stop and a pause: no length to the segment start, no moving time while it stands', () => {
        // Points every 500 m to the east; the pause starts 1 km farther.
        const at = (metres: number) => 8e6 + Math.round(metres / (metresPerDegree * Math.cos(47 * Math.PI / 180)) * 1e6);
        const t = 1_785_571_200;
        const steps: [number, number, number, boolean?][] = [[0, 0, 400], [60, 500, 403], [120, 1000, 406], [720, 1000, 406],
            [1000, 2000, 404, true], [1060, 2500, 408], [1120, 3000, 410]];
        const ride = rideSummary(steps.map(([s, m, h, pause]) => ({ timeS: t + s + .4, lat: 47e6, lon: at(m), elevationM: h, segmentStart: !s || !!pause })), 'gravel');
        expect(ride).toMatchObject({ lengthM: 2000, climbM: 12, seconds: 1120, movingS: 240, start: t, bike: 'gravel', highestM: 410 });
    });
});
