import { describe, expect, it } from 'vitest';
import { Pacer } from './pacer';

/** The display frames of one second that draw, at `hz` with a timestamp noise of ±`noise` ms. */
function drawn(hz: number, rate = 60, noise = 0): number[] {
    const pacer = new Pacer();
    pacer.rate = rate;
    const frames: number[] = [];
    for (let i = 0; i < 2 * hz; i++) {
        const now = 1000 + (i * 1000) / hz + noise * Math.sin(i * 7.1);
        if (pacer.due(now) && i >= hz) frames.push(i);
    }
    return frames;
}

const steps = (frames: number[]) => new Set(frames.slice(1).map((f, i) => f - frames[i]));

describe('Pacer', () => {
    it('draws every display frame at the rate', () => {
        expect(drawn(60)).toHaveLength(60);
        expect(steps(drawn(60, 60, 1))).toEqual(new Set([1]));
    });

    it('draws every second display frame at 120 Hz, without drift', () => {
        expect(drawn(120)).toHaveLength(60);
        expect(steps(drawn(120, 60, 1))).toEqual(new Set([2]));
    });

    it('draws a whole number of display frames apart when the rate does not divide the refresh rate', () => {
        expect(steps(drawn(144, 60, 0.5))).toEqual(new Set([2]));
        expect(steps(drawn(144, 30, 0.5))).toEqual(new Set([5]));
        expect(steps(drawn(90, 60, 0.5)).size).toBe(1);
        expect(steps(drawn(165, 30, 0.5)).size).toBe(1);
    });

    it('draws a lone frame at once', () => {
        const pacer = new Pacer();
        expect(pacer.due(0)).toBe(true);
        expect(pacer.due(5000)).toBe(true);
    });
});
