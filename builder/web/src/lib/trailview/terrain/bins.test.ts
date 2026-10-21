import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures } from '../stream/features';
import { PAGE } from '../stream/heights';
import { roadHeight } from './audit-samples';
import type { Feature } from '../stream/features';
import { CELL, EDGE, features, GRID, INSIDE, insideRings, JUNCTION, LAKE, metresPerUnit, nodeStamps, PATH, RECORD, ROAD, RULES, type Line } from './bins';

const shader = readFileSync(new URL('./stamp-core.wgsl', import.meta.url), 'utf8');
const fixture = (z: number, x: number, y: number) =>
    decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/${z}/${x}/${y}.bin`, import.meta.url)))));
/** Trübsee: the zoom-14 tile, and the level-17 node over its middle. */
const TILE = { level: 14, x: 8573, y: 5776 };
/** The entries of cell c of a node's stamp data, and the origin of its lake crossings, as stamp-core.wgsl reads them. */
function cellOf(data: Uint32Array, node: { level: number; x: number; y: number }, c: number) {
    const head = 2 + GRID * GRID + 1, offset = data[2 + c], shift = 22 - node.level, cell = CELL * 2 ** shift;
    const entries = data.subarray(head + (offset & 0xffffff), head + (data[2 + c + 1] & 0xffffff));
    const held = (v: number, first: number) => Math.min(Math.max(v, first * 2 ** shift), (first + PAGE) * 2 ** shift - 1);
    const origin = [held((node.x * PAGE - CELL) * 2 ** shift + (c % GRID) * cell + cell / 2, node.x * PAGE) + ((offset >>> 24) & 15) - 8,
        held((node.y * PAGE - CELL) * 2 ** shift + Math.floor(c / GRID) * cell + cell / 2, node.y * PAGE) + (offset >>> 28) - 8];
    return { entries, origin };
}

const lakeNode = () => {
    const { rings } = features(fixture(TILE.level, TILE.x, TILE.y), TILE).lakes[0], xs = rings[0].filter((_, i) => i % 2 === 0), ys = rings[0].filter((_, i) => i % 2 === 1);
    const middle = (values: number[]) => (Math.min(...values) + Math.max(...values)) / 2;
    return { level: 17, x: Math.floor(middle(xs) / 2 ** 13), y: Math.floor(middle(ys) / 2 ** 13) };
};

describe('the stamp data of a node', () => {
    it('gates the lines with the numbers of stamp-core.wgsl', () => {
        for (const [name, value] of Object.entries(RULES)) expect(Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(shader)?.[1]), name).toBe(value);
    });

    it('lists in each cell every segment and edge within the buffer of the feature tiles, and the lakes over its centre, in the order of the shader', () => {
        const NODE = lakeNode(), list = fixture(TILE.level, TILE.x, TILE.y), { data } = nodeStamps(list, TILE, NODE);
        const records = 2 + GRID * GRID + 1 + data![1], word = (r: number, i: number) => data![records + RECORD * r + i] | 0;
        const shift = 22 - NODE.level, cell = CELL * 2 ** shift, x0 = NODE.x * PAGE * 2 ** shift - cell, y0 = NODE.y * PAGE * 2 ** shift - cell;
        const reach = 40 / metresPerUnit(y0);
        const lake = features(list, TILE).lakes[0];
        let inside = 0;
        for (let c = 0; c < GRID * GRID; c++) {
            const { entries, origin } = cellOf(data!, NODE, c);
            const listed = new Set(Array.from(entries, entry => entry & ~INSIDE));
            const [cx0, cy0] = [x0 + (c % GRID) * cell, y0 + Math.floor(c / GRID) * cell];
            for (let r = 0; r < data![0]; r++) {
                if (![ROAD, EDGE].includes(word(r, 4))) continue;
                const [ax, ay, bx, by] = [0, 1, 2, 3].map(i => word(r, i));
                const near = Math.max(ax, bx) > cx0 - reach && Math.min(ax, bx) < cx0 + cell + reach && Math.max(ay, by) > cy0 - reach && Math.min(ay, by) < cy0 + cell + reach;
                if (near) expect(listed.has(r)).toBe(true);
            }
            // Junctions, then each lake's edges and its marker, then the segments, each line's together.
            const kinds = Array.from(entries, entry => word(entry & ~INSIDE, 4)).join('');
            expect(kinds).toMatch(new RegExp(`^${JUNCTION}*(${EDGE}*${LAKE})*[${ROAD}${PATH}]*$`));
            const lines = Array.from(entries, entry => entry & ~INSIDE).filter(r => [ROAD, PATH].includes(word(r, 4))).map(r => word(r, 13));
            expect(lines.filter((id, i) => id !== lines[i - 1]).length).toBe(new Set(lines).size);
            const marker = Array.from(entries).find(entry => word(entry & ~INSIDE, 4) === LAKE);
            if (insideRings(lake.rings, origin[0], origin[1])) {
                inside++;
                expect(marker! & INSIDE).toBe(INSIDE);
            }
        }
        expect(inside).toBeGreaterThan(0);
    });

    it('puts every texel of a cell on its side of a shore that passes through the cell\'s centre', () => {
        // A level-18 node of the Trübsee tile, and shores in every direction with a vertex on the centre of its cell (5, 9).
        const node = { level: 18, x: TILE.x * 16 + 5, y: TILE.y * 16 + 7 }, c = 9 * GRID + 5, scale = 2 ** (17 - TILE.level);
        const [cx, cy] = [(node.x * PAGE - CELL + 5 * CELL + CELL / 2) * 16 / scale - TILE.x * 8192, (node.y * PAGE - CELL + 9 * CELL + CELL / 2) * 16 / scale - TILE.y * 8192];
        for (let turn = 0; turn < 24; turn++) {
            const [cos, sin] = [Math.cos(turn * Math.PI / 12), Math.sin(turn * Math.PI / 12)];
            const shore = [[-200, -20], [0, 0], [200, 20], [200, 300], [-200, 300]].flatMap(([u, v]) => [cx + Math.round(u * cos - v * sin), cy + Math.round(u * sin + v * cos)]);
            const lake = { kind: 'lake', tags: {}, parts: [{ outer: true, xy: new Int32Array(shore), z: new Float32Array(5).fill(1764) }] } as Feature;
            const { data } = nodeStamps([lake], TILE, node), { entries, origin } = cellOf(data!, node, c);
            const records = 2 + GRID * GRID + 1 + data![1], word = (r: number, i: number) => data![records + RECORD * r + i] | 0;
            const rings = features([lake], TILE).lakes[0].rings;
            // As crosses() of stamp-core.wgsl: whether the way from the origin to p crosses an edge, half-open at the ends.
            const crosses = ([px, py]: number[], [ax, ay, bx, by]: number[]) => {
                const [ox, oy] = origin, side = (x: number, y: number) => (px - ox) * (y - oy) - (py - oy) * (x - ox) > 0;
                const edge = (x: number, y: number) => (bx - ax) * (y - ay) - (by - ay) * (x - ax) > 0;
                return side(ax, ay) !== side(bx, by) && edge(ox, oy) !== edge(px, py);
            };
            for (let j = 0; j < CELL; j++) for (let i = 0; i < CELL; i++) {
                const p = [(node.x * PAGE - CELL + 5 * CELL + i) * 16 + 8, (node.y * PAGE - CELL + 9 * CELL + j) * 16 + 8];
                let inside = false;
                for (const entry of entries) {
                    const r = entry & ~INSIDE;
                    if (word(r, 4) === EDGE) inside = inside !== crosses(p, [0, 1, 2, 3].map(k => word(r, k)));
                    else if (word(r, 4) === LAKE) inside = inside !== ((entry & INSIDE) !== 0);
                }
                expect(inside, `turn ${turn}, texel ${i}, ${j}`).toBe(insideRings(rings, p[0], p[1]));
            }
        }
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
