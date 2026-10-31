import { describe, expect, it } from 'vitest';
import { PAGE } from '../stream/heights';
import { TILE_ZOOM } from '../stream/terrain';
import { OWNER, size } from '../terrain/lod';
import { decodedGround, groundAt, groundBodies, type GroundTile } from './ground';

const SIDE = 2 * PAGE, Z = TILE_ZOOM, UNITS = 2 ** (30 - Z);
/** A tile with heights from `height` per pixel. */
const tile = (z: number, x: number, y: number, height: (i: number, j: number) => number): GroundTile => ({ z, x, y, heights: Float32Array.from({ length: SIDE * SIDE }, (_, p) => height(p % SIDE, Math.floor(p / SIDE))) });
/** The world point of the centre of pixel i, j of tile x, y of zoom z. */
const texel = (z: number, x: number, y: number, i: number, j: number) => [(x + (i + 0.5) / SIDE) * 2 ** (30 - z), (y + (j + 0.5) / SIDE) * 2 ** (30 - z)] as const;

describe('the ground of the road worker', () => {
    it('holds a tile\'s heights at its texel centres, and runs on a slope between them', () => {
        const at = groundAt([tile(Z, 10, 20, (i, j) => 1000 + i + 2 * j)]);
        expect(at(...texel(Z, 10, 20, 100, 200))).toBeCloseTo(1000 + 100 + 400, 3);
        expect(at(...texel(Z, 10, 20, 100.5, 200.25))).toBeCloseTo(1000 + 100.5 + 400.5, 3);
    });

    it('takes the deepest tile that holds a point, and knows no point past its tiles', () => {
        const at = groundAt([tile(Z - 1, 5, 10, () => 100), tile(Z, 11, 20, () => 200)]);
        expect(at(...texel(Z, 11, 20, 50, 50))).toBeCloseTo(200);
        expect(at(...texel(Z, 10, 20, 50, 50))).toBeCloseTo(100);
        expect(at(...texel(Z, 12, 20, 50, 50))).toBeNaN();
    });

    it('asks each tile about an owner once, and a coarser tile where a tile is absent', async () => {
        const owner = { level: OWNER, x: 4 * 100 + 1, y: 4 * 200 + 1 }, asked: string[] = [];
        const bodies = await groundBodies(owner, async (z, x, y) => {
            asked.push(`${z}/${x}/${y}`);
            return z === Z - 1 ? new ArrayBuffer(8) : undefined;
        });
        expect(bodies.map(({ z, x, y }) => `${z}/${x}/${y}`)).toEqual([`${Z - 1}/50/100`]);
        expect(asked).toEqual([`${Z}/100/200`, `${Z - 1}/50/100`]);
    });

    it('leaves a tile that fails without ground, not on a coarser tile', async () => {
        const owner = { level: OWNER, x: 4 * 100 + 1, y: 4 * 200 + 1 }, asked: string[] = [];
        const bodies = await groundBodies(owner, async (z, x, y) => {
            asked.push(`${z}/${x}/${y}`);
            throw new Error('A tile failed.');
        });
        expect(bodies).toEqual([]);
        expect(asked).toEqual([`${Z}/100/200`]);
    });

    it('keeps no failed decode: a tile that does not decode has no ground, and decodes anew', async () => {
        const body = { z: Z, x: 1, y: 2, body: new ArrayBuffer(8) };
        expect(await decodedGround(body)).toBeUndefined();
        expect(await decodedGround(body)).toBeUndefined();
        expect(groundAt([undefined])(0, 0)).toBeNaN();
    });

    it('reads the tile past an owner\'s edge within its margin', async () => {
        const side = size(OWNER), owner = { level: OWNER, x: 400, y: 800 };
        expect(side * 4).toBe(UNITS);
        const bodies = await groundBodies(owner, async () => new ArrayBuffer(8));
        expect(bodies.map(({ x, y }) => `${x}/${y}`).sort()).toEqual(['100/200', '100/199', '99/199', '99/200'].sort());
    });
});
