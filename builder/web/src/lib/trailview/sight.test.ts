import { describe, expect, it } from 'vitest';
import { sampleDistances, terrainHides } from './sight';

const pixel = 1 / 1100;
/** The eye stands 1000 m above a valley floor; heights are relative to the eye, as a map has them. */
const floor = -1000;

/** Whether the terrain of `profile` (height at metres across from the eye) hides a point `across` metres away at `height`. */
function hides(across: number, height: number, profile: (metres: number) => number): boolean {
    const along = sampleDistances(across);
    return terrainHides([across, 0, height], along, along.map(profile), pixel);
}

describe('terrainHides', () => {
    it('shows a point over a flat valley floor', () => {
        expect(hides(10_000, floor + 0.3, () => floor)).toBe(false);
    });

    it('hides a point on the far side of a ridge', () => {
        // The ground rises to a crest at 8 km and falls to the point behind it.
        expect(hides(10_000, floor + 0.3, m => (m < 8000 ? floor + 0.1 * m : floor + 800 - 0.4 * (m - 8000)))).toBe(true);
    });

    it('hides a point 40 km away behind a hill 100 m wide, 2 km from the eye', () => {
        // The hill's top is at the height of the eye, about 55 m above the line of sight to each point.
        for (const across of [40_000, 40_050, 40_100, 40_150]) expect(hides(across, floor, m => (Math.abs(m - 2000) < 50 ? 0 : floor))).toBe(true);
    });

    it('shows a point on a slope that faces the eye', () => {
        expect(hides(10_000, floor + 300.3, m => (m < 7000 ? floor : floor + 0.1 * (m - 7000)))).toBe(false);
    });

    it('shows a point whose own ground, within 30 m, rises over the line of sight', () => {
        expect(terrainHides([10_000, 0, floor + 0.3], [5000, 9_975], [floor, floor + 50], pixel)).toBe(false);
        expect(terrainHides([10_000, 0, floor + 0.3], [5000, 9_950], [floor, floor + 50], pixel)).toBe(true);
    });

    it('shows a point where a height is missing', () => {
        expect(hides(10_000, floor + 0.3, m => (Math.abs(m - 5000) < 500 ? NaN : floor))).toBe(false);
    });

    it('takes off the curvature of the Earth: the sea far beyond the horizon is hidden', () => {
        // The horizon of an eye 100 m above the sea is about 38 km away.
        expect(hides(150_000, -100, () => -100)).toBe(true);
        expect(hides(20_000, -100, () => -100)).toBe(false);
    });
});

it('samples the terrain more densely near the eye than far from it', () => {
    const along = sampleDistances(40_000), steps = along.slice(1).map((d, k) => d - along[k]);
    expect(steps[0]).toBeLessThan(1);
    expect(Math.max(...steps)).toBeLessThan(0.011 * 40_000);
});
