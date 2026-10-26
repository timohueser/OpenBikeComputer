import { describe, expect, it } from 'vitest';
import { dropPoint, HANG_PX, inBounds, settle, swing, viewAt } from './trailview';

describe('the hanging binoculars', () => {
    it('swing against the move and stop at 35°', () => {
        expect(swing(0, 4)).toBeGreaterThan(0);
        expect(swing(0, -4)).toBeLessThan(0);
        expect(swing(0, 500)).toBe(35);
        expect(swing(0, -500)).toBe(-35);
    });

    it('ease back to rest when the pointer stops', () => {
        expect(settle(30, 0)).toBe(30);
        expect(settle(30, 100)).toBeGreaterThan(0);
        expect(settle(30, 100)).toBeLessThan(30);
        expect(Math.abs(settle(-30, 1000))).toBeLessThan(0.1);
    });

    it('drop at their bottom, which lags behind a move to the right', () => {
        expect(dropPoint([100, 200], 0)).toEqual([100, 200 + HANG_PX]);
        const [x, y] = dropPoint([100, 200], 30);
        expect(x).toBeCloseTo(100 - HANG_PX / 2);
        expect(y).toBeCloseTo(200 + HANG_PX * Math.cos(Math.PI / 6));
    });
});

it('a point is in the bounds up to their edges', () => {
    const bounds = [5.9, 45.8, 10.5, 47.8] as const;
    expect(inBounds(bounds, [8.4, 46.8])).toBe(true);
    expect(inBounds(bounds, [10.5, 45.8])).toBe(true);
    expect(inBounds(bounds, [10.6, 46.8])).toBe(false);
    expect(inBounds(bounds, [8.4, 45.7])).toBe(false);
});

it('a view walks at noon of the day in the Alps', () => {
    // 23:30 UTC on 26 October is 00:30 on 27 October in Zurich.
    const view = viewAt([8.4, 46.8], new Date('2026-10-26T23:30:00Z'));
    expect(view).toMatchObject({ lon: 8.4, lat: 46.8, heading: 0, pitch: 0, mode: 'walk', height: 1.6, date: '2026-10-27', time: '12:00', weather: 'sunny', debug: 'off' });
});
