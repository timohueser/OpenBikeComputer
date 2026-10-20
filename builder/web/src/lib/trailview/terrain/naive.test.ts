import { describe, expect, it } from 'vitest';
import { frameAt, tileAt, tileLonLat } from '../frame/frame';
import { UNKNOWN_HEIGHT } from '../../planner/layers/terrain';
import { NaiveTerrain, surfaceAt, tileMesh, type Draw, type HeightTile } from './naive';

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

const ENGELBERG = [8.403, 46.8205] as const;
const WALK = { east: 0, north: 0, above: 1.6 };
const level = (metres: number): HeightTile => async () => ({ raster: new Int16Array(4).fill(metres), size: 2, bytes: 1 });
const never: ReturnType<HeightTile> = new Promise(() => {});

/** Selects until the terrain is idle, and lets the started loads arrive in between; undefined after `rounds` selections. */
async function settle(terrain: NaiveTerrain, rounds: number): Promise<Draw[] | undefined> {
    for (let i = 0; i < rounds; i++) {
        const draws = terrain.select(WALK);
        if (terrain.idle) return draws;
        await new Promise(resolve => setTimeout(resolve));
    }
    return undefined;
}

describe('the naive terrain selection', () => {
    it('becomes idle when the view wants more tiles than the mesh pool holds', async () => {
        const alps = new NaiveTerrain(frameAt(...ENGELBERG), level(1000), [5, 43.5, 16.5, 48.5], () => {});
        expect(await settle(alps, 400)).toBeDefined();
        expect(alps.resident).toBeGreaterThan(361);
    }, 60_000);

    it('draws each place once while a tile is missing and its children are resident', async () => {
        const z11 = tileAt(...ENGELBERG, 11), z12 = tileAt(...ENGELBERG, 12);
        const late = new Set([`11/${z11.x}/${z11.y}`, `12/${z12.x ^ 1}/${z12.y}`]);
        const heightTile: HeightTile = (z, x, y) => (late.has(`${z}/${x}/${y}`) ? never : level(1000)(z, x, y));
        const terrain = new NaiveTerrain(frameAt(...ENGELBERG), heightTile, [8.2, 46.7, 8.6, 46.95], () => {});
        await settle(terrain, 200);
        // Each quadrant drawn, as [west, north, size] in world units.
        const pieces = terrain.select(WALK).flatMap(({ tile: { z, x, y }, quadrants }) => [0, 1, 2, 3]
            .filter(q => quadrants & (1 << q)).map(q => [(x + (q & 1) / 2) / 2 ** z, (y + (q >> 1) / 2) / 2 ** z, 1 / 2 ** (z + 1)]));
        const overlaps = pieces.filter(([x, y, size], i) => pieces.some(([u, v, side], j) => i !== j
            && Math.min(x + size, u + side) > Math.max(x, u) && Math.min(y + size, v + side) > Math.max(y, v)));
        expect(overlaps).toEqual([]);
    }, 60_000);

    it('reads the ground from a coarser tile where the finer one is unknown, and never as 0 m', async () => {
        const z12 = tileAt(...ENGELBERG, 12);
        const heightTile: HeightTile = (z, x, y) => (z === 12 && x === z12.x && y === z12.y ? level(UNKNOWN_HEIGHT)(z, x, y) : level(1000)(z, x, y));
        const terrain = new NaiveTerrain(frameAt(...ENGELBERG), heightTile, [8.37, 46.765, 8.46, 46.83], () => {});
        await settle(terrain, 100);
        expect(terrain.heightAt(...ENGELBERG)).toBe(1000);
        const unknown = new NaiveTerrain(frameAt(...ENGELBERG), level(UNKNOWN_HEIGHT), [8.37, 46.765, 8.46, 46.83], () => {});
        await settle(unknown, 100);
        expect(unknown.heightAt(...ENGELBERG)).toBeUndefined();
    });
});
