import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures } from '../stream/features';
import { PAGE } from '../stream/heights';
import { roadHeight } from './audit-samples';
import type { Feature } from '../stream/features';
import { CELL, EDGE, features, gateTexel, GRID, INSIDE, insideRings, JUNCTION, LAKE, metresPerUnit, nodeStamps, PATH, smoothed, PROFILE_FLOATS, PROFILE_ROWS, PROFILES, profileTable, RECORD, RIVER, ROAD, RULES, shows, type Line, type Profile } from './bins';

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

/** As crosses() of stamp-core.wgsl: whether the way from origin o to p crosses edge a-b, with p moved by ε(1, ε). */
function crosses([ox, oy]: number[], [px, py]: number[], [ax, ay, bx, by]: number[]) {
    const left = (ux: number, uy: number, wx: number, wy: number) => ux * wy > uy * wx || (ux * wy === uy * wx && (uy < 0 || (uy === 0 && ux > 0)));
    const one = (bx - ax) * (oy - ay) - (by - ay) * (ox - ax) > 0;
    return left(ax - ox, ay - oy, px - ox, py - oy) !== left(bx - ox, by - oy, px - ox, py - oy) && one !== left(bx - ax, by - ay, px - ax, py - ay);
}

/** The gates of stamp-core.wgsl in JavaScript: its f32 constants and the functions that the gates call, as written there. */
function shaderGates(): Record<'road_shows' | 'path_shows' | 'river_shows', (h: number, ...rest: unknown[]) => boolean> {
    const constants = [...shader.matchAll(/^const (\w+): f32 = ([^;]+);/gm)].map(([, name, value]) => `const ${name} = ${value};`);
    const functions = ['reach_limit', 'gate_texel', 'river_half', 'water_bank', 'road_shows', 'path_shows', 'river_shows'].map(name => {
        const [, args, body] = new RegExp(`fn ${name}\\(([^)]*)\\) -> \\w+ \\{([^}]*)\\}`).exec(shader)!;
        return `function ${name}(${args.replace(/: \w+/g, '')}) {${body.replace(/\blet\b/g, 'const').replace(/\b(max|min)\(/g, 'Math.$1(').replace(/\bf32\(/g, '(').replace(/(\d)u\b/g, '$1')}}`;
    });
    return new Function(`${constants.join('\n')}\n${functions.join('\n')}\nreturn { road_shows, path_shows, river_shows };`)();
}

const lakeNode = () => {
    const { rings } = features(fixture(TILE.level, TILE.x, TILE.y), TILE).lakes[0], xs = rings[0].filter((_, i) => i % 2 === 0), ys = rings[0].filter((_, i) => i % 2 === 1);
    const middle = (values: number[]) => (Math.min(...values) + Math.max(...values)) / 2;
    return { level: 17, x: Math.floor(middle(xs) / 2 ** 13), y: Math.floor(middle(ys) / 2 ** 13) };
};

describe('the stamp data of a node', () => {
    it('gates the lines with the numbers of stamp-core.wgsl, and packs the profiles in the order of its struct', () => {
        for (const [name, value] of Object.entries(RULES)) expect(Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(shader)?.[1]), name).toBe(value);
        const fields = /struct Profile \{([^}]*)\}/.exec(shader)![1].match(/\w+(?=:)/g)! as (keyof Profile)[];
        const rows = Object.values(PROFILES), table = profileTable();
        expect(Number(/const PROFILE_ROWS: u32 = (\d+)u;/.exec(shader)?.[1])).toBe(PROFILE_ROWS);
        rows.forEach((row, i) => expect(Array.from(table.subarray(PROFILE_FLOATS * i, PROFILE_FLOATS * i + fields.length))).toEqual(fields.map(field => Math.fround(row[field]))));
    });

    it('shows a line on the levels where the gates of stamp-core.wgsl show it', () => {
        const gates = shaderGates(), seen = new Set<boolean>();
        for (const [kind, profile] of [[ROAD, 0], [PATH, Object.keys(PROFILES).indexOf('path')], [RIVER, 0]]) for (const h of [0.6, 2, 5, 12]) for (let level = 12; level <= 18; level++) {
            const line = { kind, h, profile } as Line, rule = Object.values(PROFILES)[profile];
            const expected = kind === RIVER ? gates.river_shows(h, level) : (kind === PATH ? gates.path_shows : gates.road_shows)(h, rule, level);
            expect(shows(line, gateTexel(level)), `kind ${kind}, h ${h}, level ${level}`).toBe(expected);
            seen.add(expected);
        }
        expect(seen.size).toBe(2);
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
            for (let j = 0; j < CELL; j++) for (let i = 0; i < CELL; i++) {
                const p = [(node.x * PAGE - CELL + 5 * CELL + i) * 16 + 8, (node.y * PAGE - CELL + 9 * CELL + j) * 16 + 8];
                let inside = false;
                for (const entry of entries) {
                    const r = entry & ~INSIDE;
                    if (word(r, 4) === EDGE) inside = inside !== crosses(origin, p, [0, 1, 2, 3].map(k => word(r, k)));
                    else if (word(r, 4) === LAKE) inside = inside !== ((entry & INSIDE) !== 0);
                }
                expect(inside, `turn ${turn}, texel ${i}, ${j}`).toBe(insideRings(rings, p[0], p[1]));
            }
        }
    });
});

describe('the side of a shore', () => {
    it('is the same from every origin for a point on the shore', () => {
        // A lake with a vertex, a straight edge and a slanted edge through the points.
        const ring = [0, 0, 100, 0, 100, 60, 40, 100, 0, 100];
        const edges = ring.map((_, i) => [ring[i], ring[i + 1], ring[(i + 2) % ring.length], ring[(i + 3) % ring.length]]).filter((_, i) => i % 2 === 0);
        const off = (x: number, y: number) => edges.every(([ax, ay, bx, by]) => {
            const t = Math.min(1, Math.max(0, ((x - ax) * (bx - ax) + (y - ay) * (by - ay)) / ((bx - ax) ** 2 + (by - ay) ** 2)));
            return Math.hypot(x - ax - t * (bx - ax), y - ay - t * (by - ay)) >= 1;
        });
        for (const p of [[0, 0], [100, 60], [50, 0], [0, 37], [70, 80]]) {
            const sides = new Set<boolean>();
            for (let oy = -40; oy <= 140; oy += 9) for (let ox = -40; ox <= 140; ox += 9) {
                if (off(ox, oy)) sides.add(edges.reduce((inside, edge) => inside !== crosses([ox, oy], p, edge), insideRings([ring], ox, oy)));
            }
            expect(sides.size, `point ${p}`).toBe(1);
        }
    });
});

describe('a rounded line', () => {
    it('keeps every vertex, and two tiles that cut it one vertex past their segments round them alike', () => {
        const xy = [0, 0, 400, 0, 800, 300, 900, 800, 1300, 1000, 1700, 900, 2000, 400], z = [1, 2, 3, 4, 5, 6, 7];
        const whole = smoothed(xy, z), at = (line: { xy: number[] }) => new Set(Array.from({ length: line.xy.length / 2 }, (_, i) => `${line.xy[2 * i]},${line.xy[2 * i + 1]}`));
        for (let v = 0; v < z.length; v++) expect(at(whole).has(`${xy[2 * v]},${xy[2 * v + 1]}`)).toBe(true);
        expect(whole.z.length).toBeGreaterThan(z.length);
        // Tile A holds vertices 0 to 4 and tile B vertices 2 to 6: A rounds segment 2-3 and B segment 3-4 as the whole line does.
        const a = smoothed(xy.slice(0, 10), z.slice(0, 5)), b = smoothed(xy.slice(4), z.slice(2));
        const between = (line: { xy: number[] }, from: number, to: number) => {
            const keys = [...at(line)], i = keys.indexOf(`${xy[2 * from]},${xy[2 * from + 1]}`), j = keys.indexOf(`${xy[2 * to]},${xy[2 * to + 1]}`);
            return keys.slice(i, j + 1);
        };
        expect(between(a, 2, 3)).toEqual(between(whole, 2, 3));
        expect(between(b, 3, 4)).toEqual(between(whole, 3, 4));
    });
});

describe('the road height of the audit', () => {
    it('runs from end to end of each segment and meets at the bisector of a bend', () => {
        // Two segments of 100 units, with a bend of 90° at (100, 0).
        const line: Line = { kind: ROAD, xy: [0, 0, 100, 0, 100, 100], z: [10, 11, 13], h: 2, code: 2, ford: false, profile: 0 };
        expect(roadHeight(line, [50, 0], 10)).toBeCloseTo(10.5);
        expect(roadHeight(line, [100, 50], 10)).toBeCloseTo(12);
        // Either side of the bisector, close to it: the height of the bend.
        expect(roadHeight(line, [96, 3.9], 10)).toBeCloseTo(11, 1);
        expect(roadHeight(line, [96, 4.1], 10)).toBeCloseTo(11, 1);
        expect(roadHeight(line, [50, 30], 10)).toBeNaN();
    });
});
