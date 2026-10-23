import { describe, expect, it } from 'vitest';
import type { Feature } from '../stream/features';
import { metresPerUnit } from '../terrain/bins';
import { BARN, CHURCH, OUTLINE, OUTLINE_WORDS, RECORD_WORDS, TEMPLATE, tileHouses } from './house-records';

/** A zoom 14 tile at Wengen; 8 world units per tile unit. */
const TILE = { level: 14, x: 8552, y: 5815 };
const building = (tag: string, xy: number[]): Feature => ({ kind: 'building', tags: { building: tag }, geometry: 'area', parts: [{ outer: true, xy: new Int32Array(xy), z: null }] });

/** A rectangle of w × d tile units about (x, y), turned by `angle`. */
function rectangle(x: number, y: number, w: number, d: number, angle: number): number[] {
    const c = Math.cos(angle), s = Math.sin(angle);
    return [[-1, -1], [1, -1], [1, 1], [-1, 1]].flatMap(([u, v]) => [Math.round(x + c * u * w / 2 - s * v * d / 2), Math.round(y + s * u * w / 2 + c * v * d / 2)]);
}

describe('the house records of a feature tile', () => {
    it('fits a turned footprint with its long side and middle, and keeps only the houses whose middle lies in the tile', () => {
        const { records, outlines } = tileHouses([
            building('church', rectangle(4000, 4000, 400, 160, 0.5)),
            building('yes', rectangle(-30, 4000, 20, 20, 0)),
        ], TILE);
        expect(records.length).toBe(RECORD_WORDS);
        expect(outlines.length).toBe(0);
        const floats = new Float32Array(records.buffer), metres = 8 * metresPerUnit(TILE.y * 8192 * 8 + 4000 * 8);
        expect(records[0] / 8 - TILE.x * 8192).toBeCloseTo(4000, 0);
        expect(records[1] / 8 - TILE.y * 8192).toBeCloseTo(4000, 0);
        // Tile y runs south: the turn is the other way in metres east and north, and the long side has two directions.
        expect(Math.abs(Math.cos(floats[2]))).toBeCloseTo(Math.cos(0.5), 2);
        expect(Math.abs(Math.sin(floats[2]))).toBeCloseTo(Math.sin(0.5), 2);
        expect(floats[3] / metres).toBeCloseTo(400, -1);
        expect(floats[4] / metres).toBeCloseTo(160, -1);
        expect(records[7] & 7).toBe(CHURCH);
        expect((records[7] >> 5) & 1).toBe(TEMPLATE);
    });

    it('covers a footprint whose sides meet at right angles with rectangles that share its seed, and draws any other along its outline', () => {
        const ell = [3000, 3000, 3100, 3000, 3100, 3030, 3030, 3030, 3030, 3100, 3000, 3100];
        const { records } = tileHouses([building('barn', ell)], TILE);
        expect(records.length).toBe(2 * RECORD_WORDS);
        expect((records[7] >> 5) & 1).toBe(TEMPLATE);
        expect(records[7] >> 16).toBe(records[RECORD_WORDS + 7] >> 16);
        const floats = new Float32Array(records.buffer), metres = 8 * metresPerUnit(TILE.y * 8192 * 8 + 3050 * 8);
        // Both arms of the L, whole: they overlap in its corner.
        expect([floats[3], floats[RECORD_WORDS + 3]].map(L => Math.round(L / metres)).sort()).toEqual([100, 100]);
        const star = [3000, 3000, 3060, 3040, 3100, 3000, 3070, 3060, 3100, 3100, 3050, 3070, 3000, 3100, 3030, 3050];
        const { records: one, outlines } = tileHouses([building('barn', star)], TILE);
        expect((one[7] >> 5) & 1).toBe(OUTLINE);
        expect(one[7] & 7).toBe(BARN);
        // Six vertices of wall per edge, and three per roof triangle.
        expect(outlines.length / OUTLINE_WORDS).toBe(6 * 8 + 3 * 6);
    });
});
