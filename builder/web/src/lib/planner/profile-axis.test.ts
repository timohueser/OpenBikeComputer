import { describe, expect, it } from 'vitest';
import { axisLeft, axisPixels, axisProgress, axisRest, type ProfileAxis } from './profile-axis';

describe('profile axis', () => {
    it('maps a pointer back to its progress across rest columns, and a point in a column to its rest', () => {
        const axis: ProfileAxis = { from: 0, to: 1, rests: [.25, .5] };
        for (const progress of [0, .1, .3, .6, 1]) expect(axisProgress(axis, axisPixels(axis, progress, 1000), 1000)).toBeCloseTo(progress);
        expect(axisPixels(axis, .5, 1000) - axisPixels(axis, .25, 1000)).toBeCloseTo(.25 * (1000 - 2 * 64) + 64);
        expect(axisProgress(axis, axisPixels(axis, .25, 1000) + 30, 1000)).toBe(.25);
        // The CSS position says the same: past both columns at three quarters, one column after the first rest.
        expect(axisLeft(axis, .75)).toBe('calc(75% + 0.5 * min(64px, 25%))');
        expect(axisLeft(axis, .25, true)).toBe('calc(25% + 0.5 * min(64px, 25%))');
    });

    it('keeps the dragged day end under the pointer while its rest column follows it', () => {
        const axis: ProfileAxis = { from: 0, to: 1, rests: [.25, .5] };
        for (const progress of [.4, .55]) {
            const moved = { ...axis, rests: [.25, progress] };
            expect(axisProgress(axis, axisPixels(moved, progress, 1000), 1000, .5)).toBeCloseTo(progress);
        }
    });

    it('narrows the columns to half the track together', () => {
        const axis: ProfileAxis = { from: 0, to: 1, rests: [.1, .3, .5, .7, .9] };
        expect(axisPixels(axis, .3, 315) - axisPixels(axis, .1, 315)).toBeCloseTo(.2 * 315 / 2 + 315 / 10);
        expect(axisProgress(axis, axisPixels(axis, .6, 315), 315)).toBeCloseTo(.6);
        // Two rest days after one night stand side by side.
        expect(axisRest({ from: 0, to: 1, rests: [.5, .5] }, 1)).toBe('calc(50% + 0 * min(64px, 25%))');
    });
});
