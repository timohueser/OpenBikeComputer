import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { metresPerUnit, PATH, PROFILES, ROAD, walkOf, type Line, type Shapes } from '../terrain/bins';
import { RIGHT } from '../terrain/town';
import { coreAt, coresOf } from './cores';
import { CF, sideOf } from './section';

describe('the cores of the ways', () => {
    it('hold a sidewalk from the step that ends where it starts, as the sweep draws it, and round the end of a way', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y);
        // A residential road east, 2.5 m each side; a sidewalk on its right (south) from its middle vertex on.
        const line = { kind: ROAD, xy: [-50 * m, y, 0, y, 50 * m, y], z: [0, 0, 0], h: 2.5, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'), sidewalks: [0, RIGHT] } as Line;
        const cores = coresOf({ lines: [line], lakes: [], seas: [], pads: [], areas: [], buildings: [] } as unknown as Shapes), across = 2.5 + walkOf(line) - 0.2;
        expect(across).toBeGreaterThan(2.9);
        expect(coreAt(cores, 0.5 * m, y + across * m)?.side).toBe(-1);
        expect(coreAt(cores, -1 * m, y + across * m)).toBeUndefined();
        // A dead end has a round cap, as the sweep draws it.
        expect(coreAt(cores, 51 * m, y)).toBeDefined();
        expect(coreAt(cores, 52 * m, y + 2 * m)).toBeDefined();
        expect(coreAt(cores, 53 * m, y)).toBeUndefined();
        // `past` widens the core.
        expect(coreAt(cores, -1 * m, y + across * m, 1)).toBeDefined();
        // A sidewalk that ends at the middle vertex: the step after it lies on the next segment.
        const ends = { ...line, sidewalks: [RIGHT, 0] };
        const ended = coresOf({ lines: [ends], lakes: [], seas: [], pads: [], areas: [], buildings: [] } as unknown as Shapes);
        expect(coreAt(ended, -0.5 * m, y + across * m)?.side).toBe(-1);
        expect(coreAt(ended, 0.5 * m, y + across * m)).toBeUndefined();
    });

    it('hold a point on a way\'s curve that lies far from its vertices', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y), turn = Math.PI / 4, flat = [0, 0];
        // A road of two vertices 100 m apart that leaves east by north and comes in east by south: its curve passes 17.7 m north of its middle.
        const line = { kind: ROAD, xy: [-50 * m, y, 50 * m, y], z: flat, h: 2.5, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'), plan: { tangent: [-turn, turn], grade: flat, bank: flat, widen: flat } } as Line;
        const cores = coresOf({ lines: [line], lakes: [], seas: [], pads: [], areas: [], buildings: [] } as unknown as Shapes);
        expect(coreAt(cores, 0, y - 17.7 * m)).toBeDefined();
        expect(coreAt(cores, 0, y)).toBeUndefined();
    });

    it('give the stand of points.wgsl the band rows that the corridor draws', () => {
        const wgsl = readFileSync(new URL('./band.wgsl', import.meta.url), 'utf8');
        const rows = (name: string) => [...new RegExp(`const ${name} = vec3f\\(([^)]*)\\);`).exec(wgsl)![1].split(',').map(Number)];
        const sampled = (kind: number) => {
            const { parts, edge } = sideOf({ kind, xy: [], z: [], h: 1, code: 2, ford: false, profile: 0 } as Line, 1, 0, 1);
            return parts.filter(({ flags }) => flags === CF.SAMPLE).map(({ a }) => Math.round((a - edge) * 100) / 100);
        };
        expect(rows('BAND_ROWS_M')).toEqual(sampled(ROAD));
        expect([...new Set(rows('PATH_BAND_ROWS_M'))]).toEqual(sampled(PATH));
    });
});
