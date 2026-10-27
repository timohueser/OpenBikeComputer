import { describe, expect, it } from 'vitest';
import { DARK_SKY, nightSky } from './night';

const MIDNIGHT = { sun: -40, moon: -30, phase: 180, sky: DARK_SKY, cover: 0, left: 1 };

describe('the night sky', () => {
    it('shows the faint stars and the Milky Way far from lights, and only bright stars under a town sky', () => {
        const dark = nightSky(MIDNIGHT), town = nightSky({ ...MIDNIGHT, sky: 18.5 });
        expect(dark.limit).toBeGreaterThan(6.3);
        expect(dark.milkyWay).toBeGreaterThan(0.95);
        expect(town.limit).toBeLessThan(4.5);
        expect(town.milkyWay).toBe(0);
    });

    it('lets a high full moon wash out the Milky Way and the faint stars', () => {
        const full = nightSky({ ...MIDNIGHT, moon: 50, phase: 0 }), half = nightSky({ ...MIDNIGHT, moon: 50, phase: 90 });
        expect(full.limit).toBeLessThan(5);
        expect(full.milkyWay).toBe(0);
        expect(half.limit).toBeGreaterThan(full.limit + 1);
    });

    it('shows the first stars in the blue hour, and all of them once the sun is 18° down', () => {
        expect(nightSky({ ...MIDNIGHT, sun: -5 }).limit).toBeLessThan(0);
        expect(nightSky({ ...MIDNIGHT, sun: -9 }).limit).toBeGreaterThan(2);
        expect(nightSky({ ...MIDNIGHT, sun: -18 }).limit).toBeCloseTo(nightSky(MIDNIGHT).limit, 1);
    });
});
