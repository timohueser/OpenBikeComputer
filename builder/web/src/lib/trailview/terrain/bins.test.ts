import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures } from '../stream/features';
import { PAGE } from '../stream/heights';
import { roadHeight } from './audit-samples';
import { CELL, EDGE, features, GRID, INSIDE, insideRings, LAKE, metresPerUnit, nodeStamps, RECORD, ROAD, RULES, type Line } from './bins';

const shader = readFileSync(new URL('./stamp-core.wgsl', import.meta.url), 'utf8');
const fixture = (z: number, x: number, y: number) =>
    decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/${z}/${x}/${y}.bin`, import.meta.url)))));
/** Trübsee: the zoom-14 tile, and the level-17 node over its middle. */
const TILE = { level: 14, x: 8573, y: 5776 };
const lakeNode = () => {
    const { rings } = features(fixture(TILE.level, TILE.x, TILE.y), TILE).lakes[0], xs = rings[0].filter((_, i) => i % 2 === 0), ys = rings[0].filter((_, i) => i % 2 === 1);
    const middle = (values: number[]) => (Math.min(...values) + Math.max(...values)) / 2;
    return { level: 17, x: Math.floor(middle(xs) / 2 ** 13), y: Math.floor(middle(ys) / 2 ** 13) };
};

describe('the stamp data of a node', () => {
    it('gates the lines with the numbers of stamp-core.wgsl', () => {
        for (const [name, value] of Object.entries(RULES)) expect(Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(shader)?.[1]), name).toBe(value);
    });

    it('lists in each cell every segment and edge within the buffer of the feature tiles, and the lakes over its centre', () => {
        const NODE = lakeNode(), list = fixture(TILE.level, TILE.x, TILE.y), { data } = nodeStamps(list, TILE, NODE);
        const records = 2 + GRID * GRID + 1 + data![1], word = (r: number, i: number) => data![records + RECORD * r + i] | 0;
        const shift = 22 - NODE.level, cell = CELL * 2 ** shift, x0 = NODE.x * PAGE * 2 ** shift - cell, y0 = NODE.y * PAGE * 2 ** shift - cell;
        const reach = 40 / metresPerUnit(y0);
        const lake = features(list, TILE).lakes[0];
        let inside = 0;
        for (let c = 0; c < GRID * GRID; c++) {
            const entries = data!.subarray(2 + GRID * GRID + 1 + data![2 + c], 2 + GRID * GRID + 1 + data![2 + c + 1]);
            const listed = new Set(Array.from(entries, entry => entry & ~INSIDE));
            const [cx0, cy0] = [x0 + (c % GRID) * cell, y0 + Math.floor(c / GRID) * cell];
            for (let r = 0; r < data![0]; r++) {
                if (![ROAD, EDGE].includes(word(r, 4))) continue;
                const [ax, ay, bx, by] = [0, 1, 2, 3].map(i => word(r, i));
                const near = Math.max(ax, bx) > cx0 - reach && Math.min(ax, bx) < cx0 + cell + reach && Math.max(ay, by) > cy0 - reach && Math.min(ay, by) < cy0 + cell + reach;
                if (near) expect(listed.has(r)).toBe(true);
            }
            const marker = Array.from(entries).find(entry => word(entry & ~INSIDE, 4) === LAKE);
            if (insideRings(lake.rings, cx0 + cell / 2, cy0 + cell / 2)) {
                inside++;
                expect(marker! & INSIDE).toBe(INSIDE);
            }
        }
        expect(inside).toBeGreaterThan(0);
    });
});

describe('the road height of the audit', () => {
    it('runs from end to end of each segment and meets at the bisector of a bend', () => {
        // Two segments of 100 units, with a bend of 90° at (100, 0).
        const line: Line = { kind: ROAD, xy: [0, 0, 100, 0, 100, 100], z: [10, 11, 13], h: 2, code: 2, ford: false, closed: false };
        expect(roadHeight(line, [50, 0], 10)).toBeCloseTo(10.5);
        expect(roadHeight(line, [100, 50], 10)).toBeCloseTo(12);
        // Either side of the bisector, close to it: the height of the bend.
        expect(roadHeight(line, [96, 3.9], 10)).toBeCloseTo(11, 1);
        expect(roadHeight(line, [96, 4.1], 10)).toBeCloseTo(11, 1);
        expect(roadHeight(line, [50, 30], 10)).toBeNaN();
    });
});
