import { describe, expect, it } from 'vitest';
import { frameAt, tileLonLat } from '../frame/frame';
import { UNKNOWN_HEIGHT } from '../../planner/layers/terrain';
import { surfaceAt, tileMesh } from './naive';

const SIZE = 512, GRID = 128;

describe('the naive terrain mesh', () => {
    // A raster that rises 1 m per pixel to the east, with an unknown north-west corner.
    const raster = Int16Array.from({ length: SIZE * SIZE }, (_, k) => (k % SIZE < 8 && k < 8 * SIZE ? UNKNOWN_HEIGHT : 1000 + (k % SIZE)));
    const frame = frameAt(...tileLonLat(2143.5, 1443.5, 12));
    const tile = tileMesh(frame, 12, 2143, 1443, raster, SIZE);

    it('samples pixel centres, and the eye walks on the drawn triangles', () => {
        // Grid column 64 lies on the edge between pixels 255 and 256.
        expect(surfaceAt(tile.grid, 64, 64)).toBeCloseTo(1255.5, 4);
        expect(surfaceAt(tile.grid, 64.25, 64.5)).toBeCloseTo(1256.5, 4);
        expect(surfaceAt(tile.grid, GRID, 64)).toBe(1511);
        expect(surfaceAt(tile.grid, 0.5, 0.5)).toBeUndefined();
    });

    it('places vertices about the tile centre with normals that face up the slope', () => {
        const floats = new Float32Array(tile.vertices), normals = new Int8Array(tile.vertices);
        const k = 64 * (GRID + 1) + 64;
        expect(Math.hypot(floats[4 * k], floats[4 * k + 1])).toBeLessThan(0.01);
        expect(floats[4 * k + 2]).toBeCloseTo(1255.5, 3);
        const [x, y, z, known] = normals.slice(16 * k + 12, 16 * k + 16);
        expect(x).toBeLessThan(0);
        expect(Math.abs(y)).toBeLessThanOrEqual(1);
        expect(z).toBeGreaterThan(0);
        expect(known).toBe(127);
        expect(normals[12 + 3]).toBe(0);
    });
});
