import { describe, expect, it } from 'vitest';
import { terrainHides } from './sight';

/** The terrain heights at the samples of a line of sight to a point `across` metres away, from a profile in metres along it. */
const profile = (across: number, height: (metres: number) => number, samples = 64) =>
    Array.from({ length: samples }, (_, k) => height(across * (k + 1) / (samples + 1)));
const pixel = 1 / 1500;

describe('terrainHides', () => {
    // The eye is 1000 m above a valley floor; the point lies on the floor 10 km away.
    const floor = -1000, point = [10_000, 0, floor + 0.3] as const;

    it('shows a point over flat ground: trees and houses are not in the terrain', () => {
        expect(terrainHides(point, profile(10_000, () => floor), pixel)).toBe(false);
    });

    it('hides a point behind a ridge between the eye and the point', () => {
        expect(terrainHides(point, profile(10_000, m => (Math.abs(m - 5000) < 400 ? floor + 800 : floor)), pixel)).toBe(true);
    });

    it('hides a point on the far side of a ridge', () => {
        // The ground rises to a crest at 8 km and falls to the point behind it.
        const ridge = (m: number) => (m < 8000 ? floor + 0.1 * m : floor + 800 - 0.4 * (m - 8000));
        expect(terrainHides(point, profile(10_000, ridge), pixel)).toBe(true);
    });

    it('shows a point on a slope that faces the eye', () => {
        // The ground rises toward the point, which stands 300 m up the slope, just under the line of sight.
        const to = [10_000, 0, floor + 300.3] as const;
        const slope = (m: number) => (m < 7000 ? floor : floor + 0.1 * (m - 7000));
        expect(terrainHides(to, profile(10_000, slope), pixel)).toBe(false);
    });
});
