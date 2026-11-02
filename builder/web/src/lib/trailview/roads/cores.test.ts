import { describe, expect, it } from 'vitest';
import { metresPerUnit, PROFILES, ROAD, walkOf, type Line, type Shapes } from '../terrain/bins';
import { RIGHT } from '../terrain/town';
import { coreAt, coresOf } from './cores';

describe('the cores of the ways', () => {
    it('hold a sidewalk from the step that ends where it starts, as the sweep draws it, and stop at the end of a way', () => {
        const y = 2 ** 29, m = 1 / metresPerUnit(y);
        // A residential road east, 2.5 m each side; a sidewalk on its right (south) from its middle vertex on.
        const line = { kind: ROAD, xy: [-50 * m, y, 0, y, 50 * m, y], z: [0, 0, 0], h: 2.5, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'), sidewalks: [0, RIGHT] } as Line;
        const cores = coresOf({ lines: [line], lakes: [], seas: [], areas: [], buildings: [] } as unknown as Shapes), across = 2.5 + walkOf(line) - 0.2;
        expect(across).toBeGreaterThan(2.9);
        expect(coreAt(cores, 0.5 * m, y + across * m)?.side).toBe(-1);
        expect(coreAt(cores, -1 * m, y + across * m)).toBeUndefined();
        expect(coreAt(cores, 51 * m, y)).toBeUndefined();
        // `past` widens the core.
        expect(coreAt(cores, -1 * m, y + across * m, 1)).toBeDefined();
    });
});
