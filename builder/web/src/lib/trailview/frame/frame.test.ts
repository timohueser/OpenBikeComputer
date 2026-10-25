import { describe, expect, it } from 'vitest';
import { EARTH_RADIUS, frameAt } from './frame';

describe('the local metric frame', () => {
    const frame = frameAt(8.4, 46.8);

    it('keeps distances from its origin, and returns a point 300 km away to the same place', () => {
        const [east, north] = frame.local(8.4, 47.8);
        expect(east).toBeCloseTo(0, 6);
        expect(north).toBeCloseTo(EARTH_RADIUS * Math.PI / 180, 3);
        const [x, y] = frame.local(12.5, 46.8);
        const angle = Math.acos(Math.sin(46.8 * Math.PI / 180) ** 2 + Math.cos(46.8 * Math.PI / 180) ** 2 * Math.cos(4.1 * Math.PI / 180));
        expect(Math.hypot(x, y)).toBeCloseTo(EARTH_RADIUS * angle, 3);

        const [lon, lat] = frame.lonLat(...frame.local(11.9, 45.1));
        expect(lon).toBeCloseTo(11.9, 9);
        expect(lat).toBeCloseTo(45.1, 9);
        expect(frame.lonLat(0, 0)).toEqual([8.4, 46.8]);
    });
});
