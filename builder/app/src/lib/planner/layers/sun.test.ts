import { describe, expect, it } from 'vitest';
import { coordinateAt, instantAt, nativePoint, patchBlocks, solarPosition, solarTerms, visibility, horizonVisibility, mapVisibility, SUN, SHADE, NIGHT, UNKNOWN, OUTSIDE, type Surface } from './sun';

const RAD = Math.PI / 180;
describe('sun position and region clock', () => {
    it('matches geometric NREL SPA positions through the seasons', () => {
        for (const [date, bearing, altitude] of [
            ['2026-06-21T07:00:00Z', 89.545571, 32.042599],
            ['2026-12-21T11:00:00Z', 173.65359, 18.486956],
            ['2026-03-20T05:30:00Z', 89.04318, -1.074814],
        ] as const) {
            const position = solarPosition([7.95, 47.83], solarTerms(Date.parse(date)));
            expect(Math.abs(position.bearing / RAD - bearing)).toBeLessThan(.02);
            expect(Math.abs(position.altitude / RAD - altitude)).toBeLessThan(.02);
        }
    });
    it('uses the region DST rather than the computer timezone, and rejects a clock gap', () => {
        expect(new Date(instantAt('2026-06-21', 540, 'Europe/Berlin')).toISOString()).toBe('2026-06-21T07:00:00.000Z');
        expect(new Date(instantAt('2026-12-21', 540, 'Europe/Berlin')).toISOString()).toBe('2026-12-21T08:00:00.000Z');
        expect(() => instantAt('2026-03-29', 150, 'Europe/Berlin')).toThrow('does not exist');
    });
});

describe('terrain visibility', () => {
    it('uses day/night at wide zooms without terrain requests and clips the region edge', () => {
        const bounds: [number, number, number, number] = [7, 47, 11, 51];
        const sun = { bearing: 0, altitude: .1 };
        const unavailable = () => { throw new Error('Terrain must not load'); };
        expect(mapVisibility([8, 48], sun, 6, bounds, unavailable)).toBe(SUN);
        expect(mapVisibility([0, 48], sun, 6, bounds, unavailable)).toBe(SUN);
        expect(mapVisibility([0, 48], { ...sun, altitude: -.1 }, 6, bounds, unavailable)).toBe(NIGHT);
        expect(mapVisibility([8, 48], { ...sun, altitude: -.1 }, 12, bounds, unavailable)).toBe(NIGHT);
        expect(mapVisibility([6, 48], sun, 8, bounds, unavailable)).toBe(OUTSIDE);
        expect(mapVisibility([8, 48], sun, 7, bounds, () => SHADE)).toBe(SHADE);
        expect(mapVisibility([8, 48], sun, 7, bounds, () => UNKNOWN)).toBe(UNKNOWN);
    });
    it('interpolates baked horizons across north and grid edges without hiding unknown terrain', () => {
        const sun = { bearing: 359 * RAD, altitude: 12 * RAD };
        const read = (x: number, y: number, d: number) => d === 0 ? 10 + x + y : 14 + x + y;
        expect(horizonVisibility(read, .5, .5, sun, 72, 1)).toBe(SUN);
        expect(horizonVisibility(read, .5, .5, { ...sun, altitude: 10 * RAD }, 72, 1)).toBe(SHADE);
        expect(horizonVisibility(() => 255, .5, .5, sun, 72, 1)).toBe(UNKNOWN);
        expect(horizonVisibility(() => { throw new Error('Night needs no tiles'); }, 0, 0, { ...sun, altitude: -1 }, 72, 1)).toBe(NIGHT);
    });
    it('checks the interior of a bilinear patch, where both endpoints are below the ray', () => {
        expect(patchBlocks([0, 10, 10, 0], 0, 0, 1, 1, 0, 4, 0, 1)).toBe(true);
        expect(patchBlocks([0, 10, 10, 0], 0, 0, 1, 1, 0, 6, 0, 1)).toBe(false);
    });
    it('keeps half-pixel registration and distinguishes night, shade and missing terrain', () => {
        const coordinate = coordinateAt(1096000.75, 730000.75, 12, 512);
        const [x, y] = nativePoint(coordinate);
        expect(x).toBeCloseTo(1096000.25, 7); expect(y).toBeCloseTo(730000.25, 7);
        const flat: Surface = { height: () => 100 };
        expect(visibility(flat, coordinate, { bearing: 0, altitude: .1 }, 30000)).toBe(SUN);
        expect(visibility(flat, coordinate, { bearing: 0, altitude: -.1 }, 30000)).toBe(NIGHT);
        expect(visibility({ height: () => Infinity }, coordinate, { bearing: 0, altitude: .1 }, 30000)).toBe(UNKNOWN);
        const ridge: Surface = { height: (level, _x, row) => level ? 500 : row < 729990 ? 500 : 100 };
        expect(visibility(ridge, coordinate, { bearing: 0, altitude: .1 }, 30000)).toBe(SHADE);
    });
    it('matches a traversal without culling over ridges, tile edges and low sun', () => {
        const origin = [1096000, 730000];
        const vertex = (x: number, y: number) => 400 + 180 * Math.sin((x - origin[0]) / 7) * Math.cos((y - origin[1]) / 11);
        const bounds = new Map<string, number>();
        const surface: Surface = { height(level, x, y) {
            if (!level) return vertex(x, y);
            if (level > 5) return 580;
            const key = `${level}/${x}/${y}`, known = bounds.get(key);
            if (known !== undefined) return known;
            let maximum = -Infinity;
            for (let row = y * 2 ** level; row <= (y + 1) * 2 ** level; row++)
                for (let col = x * 2 ** level; col <= (x + 1) * 2 ** level; col++) maximum = Math.max(maximum, vertex(col, row));
            bounds.set(key, maximum); return maximum;
        } };
        for (let i = 0; i < 256; i++) {
            const point = coordinateAt(origin[0] + (i * 73 % 512) + .2, origin[1] + (i * 127 % 512) + .8, 12, 512);
            const sun = { bearing: (i * 137.508 % 360) * RAD, altitude: (.1 + (i * 19 % 60)) * RAD };
            expect(visibility(surface, point, sun, 3000)).toBe(visibility(surface, point, sun, 3000, undefined, false));
        }
    });
});
