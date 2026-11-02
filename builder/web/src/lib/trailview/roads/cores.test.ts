import { describe, expect, it } from 'vitest';
import { metresPerUnit, PROFILES, ROAD, type Line } from '../terrain/bins';
import { LEFT } from '../terrain/town';
import { coreAt, coreEdge, coresOf, stationsIn } from './cores';

describe('the core of a way', () => {
    // A residential road 100 m east along world row 2^29, with a sidewalk on its left (north).
    const metres = metresPerUnit(2 ** 29), zero = [0, 0];
    const line: Line = {
        kind: ROAD, xy: [2 ** 28, 2 ** 29, 2 ** 28 + Math.round(100 / metres), 2 ** 29], z: [1000, 1000], h: 3, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'),
        anchors: [true, true], plan: { tangent: zero, grade: zero, bank: zero, widen: zero }, sidewalks: [LEFT],
    };
    const cores = coresOf([line]), stations = stationsIn(cores, line), k = Math.floor(stations.length / 2), s = stations[k];
    const at = (across: number, ahead = 0) => [s.x + s.ax * across - s.ay * ahead, s.y + s.ay * across + s.ax * ahead] as const;

    it('reaches out to the edge of each side, past the sidewalk on the left', () => {
        const [left, right] = [coreEdge(line, s, s, 1), coreEdge(line, s, s, -1)];
        expect(left).toBeGreaterThan(right);
        expect(right).toBeGreaterThanOrEqual(line.h);
        for (const [across, side] of [[left - 0.05, 1], [-right + 0.05, -1]] as const) expect(coreAt(cores, cores.ways, ...at(across))?.side).toBe(side);
        for (const across of [left + 0.05, -right - 0.05]) expect(coreAt(cores, cores.ways, ...at(across)), `${across} m across`).toBeUndefined();
        expect(coreAt(cores, cores.ways, ...at(left + 0.05), 0.1)?.side).toBe(1);
    });

    it('stops within its width past its ends', () => {
        for (const end of [stations[0], stations[stations.length - 1]]) {
            const out = end === stations[0] ? -1 : 1, edge = coreEdge(line, end, end, 1), ahead = (metres: number) => [end.x - end.ay * metres * out, end.y + end.ax * metres * out] as const;
            expect(coreAt(cores, cores.ways, ...ahead(-1))).toBeDefined();
            expect(coreAt(cores, cores.ways, ...ahead(edge + 0.05))).toBeUndefined();
        }
    });
});
