import { describe, expect, it } from 'vitest';
import type { Feature } from '../stream/features';
import { orchardRows, ROW_M, STEP_M } from './orchard-rows';

const UNITS = 2 ** 30 / 2 ** 14 / 8192;

describe('the orchard rows', () => {
    it('fill an orchard with rows along its longest side, keep out of its hole, and give the trees of the tile itself', () => {
        // A tile at 47° N: about 1.67 m per 8 tile units; a 400 × 1200 tile unit orchard with a hole.
        const ring = (x0: number, y0: number, x1: number, y1: number) => Int32Array.from([x0, y0, x1, y0, x1, y1, x0, y1]);
        const orchard: Feature = { kind: 'orchard', tags: { trees: 'apple_trees' }, geometry: 'area', parts: [
            { outer: true, xy: ring(1000, 1000, 1400, 2200), z: null }, { outer: false, xy: ring(1100, 1100, 1300, 1300), z: null },
        ] };
        const berries: Feature = { ...orchard, tags: { produce: 'blueberry' } };
        const edge: Feature = { ...orchard, parts: [{ outer: true, xy: ring(8000, 1000, 8400, 1400), z: null }] };
        const tile = { x: 8530, y: 5770 };
        const trees = orchardRows([orchard, berries, edge], tile);
        const local = Array.from({ length: trees.length / 2 }, (_, i) => [trees[2 * i] / UNITS - tile.x * 8192, trees[2 * i + 1] / UNITS - tile.y * 8192]);
        expect(local.every(([x, y]) => x >= 1000 && x <= 8192 && y >= 1000 && y <= 2200)).toBe(true);
        expect(local.some(([x, y]) => x > 1100 && x < 1300 && y > 1100 && y < 1300)).toBe(false);
        // The rows run along y, the long side: few distinct x, many distinct y.
        const columns = new Set(local.filter(([x]) => x < 2000).map(([x]) => Math.round(x / 10)));
        const metres = 2 * Math.PI * 6371008.8 * Math.cos(47 * Math.PI / 180) / 2 ** 14 / 8192;
        expect(columns.size).toBeCloseTo(400 * metres / ROW_M, -0.5);
        expect(local.filter(([x]) => x < 2000).length).toBeGreaterThan((400 * 1200 - 200 * 200) * metres * metres / (ROW_M * STEP_M) * 0.7);
    });
});
