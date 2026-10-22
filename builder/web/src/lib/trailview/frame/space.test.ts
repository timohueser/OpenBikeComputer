import { describe, expect, it } from 'vitest';
import { frameAt, tileLonLat } from './frame';
import { anchorAt, cubic, renderMap } from './space';

describe('LOD space and render space', () => {
    // The frame of the first view, and an eye 40 km away from its origin.
    const frame = frameAt(8.403, 46.8205);
    const anchor = anchorAt(8.9, 46.6), map = renderMap(frame, anchor);
    const [eyeEast, eyeNorth] = frame.local(8.9, 46.6);
    const error = (distance: number) => Math.max(...[0, 1, 2, 3, 4, 5, 6, 7].map(k => {
        const bearing = k * Math.PI / 4, s: [number, number] = [distance * Math.sin(bearing), distance * Math.cos(bearing)];
        const [east, north] = frame.local(...tileLonLat(anchor.whole[0] + anchor.part[0] + s[0] / anchor.unit, anchor.whole[1] + anchor.part[1] - s[1] / anchor.unit, 30));
        return Math.hypot(cubic(map, ...s, 0) - (east - eyeEast), cubic(map, ...s, 10) - (north - eyeNorth));
    }));

    it('places the terrain where the frame places a point, to 3 mm within 30 km and 5 m at 200 km', () => {
        expect(error(3_000)).toBeLessThan(1e-5);
        expect(error(30_000)).toBeLessThan(3e-3);
        expect(error(200_000)).toBeLessThan(5);
    });
});
