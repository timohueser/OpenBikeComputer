import { describe, expect, it } from 'vitest';
import { jitter, MOVING_WEIGHT, STILL_FRAMES, Temporal } from './temporal';

describe('the jitter', () => {
    it('is Halton 2, 3 about the pixel centre', () => {
        const expected = [[0, -1 / 6], [-0.25, 1 / 6], [0.25, -7 / 18], [-0.375, -1 / 18]];
        expected.forEach((sample, k) => sample.forEach((value, axis) => expect(jitter(k + 1)[axis]).toBeCloseTo(value, 12)));
    });

    it('covers the pixel evenly over the frames of a still view', () => {
        const samples = Array.from({ length: STILL_FRAMES }, (_, k) => jitter(k + 1));
        for (const axis of [0, 1]) {
            const values = samples.map(sample => sample[axis]);
            expect(Math.abs(values.reduce((a, b) => a + b) / STILL_FRAMES)).toBeLessThan(0.05);
            expect(new Set(values.map(v => Math.floor((v + 0.5) * 4))).size).toBe(4);
        }
    });
});

describe('the accumulation', () => {
    const still = (temporal: Temporal, changed = false) => temporal.next(false, changed);

    it('weighs frame k of a still view 1/k and draws no frame after the last', () => {
        const temporal = new Temporal();
        const weights = Array.from({ length: STILL_FRAMES }, () => still(temporal)?.weight);
        expect(weights).toEqual(Array.from({ length: STILL_FRAMES }, (_, k) => 1 / (k + 1)));
        expect(temporal.converging).toBe(false);
        expect(still(temporal)).toBeUndefined();
    });

    it('starts again when the camera stops and when the content of a still view changes', () => {
        const temporal = new Temporal();
        for (let k = 0; k < 5; k++) still(temporal);
        expect(still(temporal, true)).toEqual({ weight: 1, sample: 1, clip: false });
        expect(temporal.next(true, false)).toMatchObject({ weight: MOVING_WEIGHT, clip: true });
        expect(temporal.next(true, true)).toMatchObject({ weight: MOVING_WEIGHT, clip: true });
        expect(temporal.converging).toBe(true);
        expect(still(temporal)).toEqual({ weight: 1, sample: 1, clip: false });
    });

    it('drops the history after a reset, while the camera moves too', () => {
        const temporal = new Temporal();
        temporal.next(true, false);
        temporal.reset();
        expect(temporal.next(true, false)?.weight).toBe(1);
        expect(temporal.next(true, false)?.weight).toBe(MOVING_WEIGHT);
    });

    it('takes one frame in a view without jitter', () => {
        const temporal = new Temporal();
        expect(temporal.next(false, false, 1)?.weight).toBe(1);
        expect(temporal.converging).toBe(false);
        expect(temporal.next(false, false, 1)).toBeUndefined();
    });
});
