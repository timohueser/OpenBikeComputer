import { describe, expect, it } from 'vitest';
import { worldPixel } from '../../planner/layers/mercator';
import { frameAt, tileLonLat } from './frame';
import { anchorAt, cubic, lodSpace, renderMap, UNITS } from './space';

describe('LOD space and render space', () => {
    // The frame of the first view, and an eye 40 km away from its origin.
    const frame = frameAt(8.403, 46.8205);
    const anchor = anchorAt(8.9, 46.6), map = renderMap(frame, anchor);
    const [eyeEast, eyeNorth] = frame.local(8.9, 46.6);
    const error = (distance: number) => Math.max(...[0, 1, 2, 3, 4, 5, 6, 7].map(k => {
        const bearing = k * Math.PI / 4, s: [number, number] = [distance * Math.sin(bearing), distance * Math.cos(bearing)];
        const [east, north] = frame.local(...tileLonLat(anchor.whole[0] + anchor.part[0] + s[0] / anchor.unit, anchor.whole[1] + anchor.part[1] - s[1] / anchor.unit, 30));
        const x = cubic(map.forward, ...s, 0), y = cubic(map.forward, ...s, 10);
        const back = [cubic(map.inverse, x, y, 0), cubic(map.inverse, x, y, 10)];
        return Math.max(Math.hypot(x - (east - eyeEast), y - (north - eyeNorth)), Math.hypot(back[0] - s[0], back[1] - s[1]));
    }));

    it('splits the eye into whole units and a part, at the scale of its latitude', () => {
        const [x, y] = worldPixel([8.9, 46.6], UNITS);
        expect(anchor.whole[0] + anchor.part[0]).toBe(x);
        expect(anchor.whole[1] + anchor.part[1]).toBe(y);
        expect(anchor.part.every(part => part >= 0 && part < 1)).toBe(true);
        // A unit is about 2.6 cm, and LOD space is metres north up.
        expect(anchor.unit).toBeCloseTo(0.02559, 4);
        expect(lodSpace(anchor, x + 100, y - 100)).toEqual([100 * anchor.unit, 100 * anchor.unit]);
    });

    it('places the terrain where the frame places a point, and back, to 3 mm within 30 km and 5 m at 200 km', () => {
        expect(error(3_000)).toBeLessThan(1e-5);
        expect(error(30_000)).toBeLessThan(3e-3);
        expect(error(200_000)).toBeLessThan(5);
    });
});
