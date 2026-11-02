import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { functionOf, shaderFunctions } from '../gpu/shader-functions';
import { decodeFeatures, EXTENT, SIDE_BACK, SIDE_ON, unitScale, type Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import { auditSamples, IN_WATER, roadHeight, SAMPLE } from './audit-samples';
import { BRIDGE, CELL, cellAt, CORNER, crosses, crossSlope, EDGE, features, gateTexel, GRID, halfWidth, INSIDE, insideRings, isPath, JUNCTION, junctions, junctionWays, LAKE, metresPerUnit, NEAR, nodeStamps, PATH, ringsDistance, smoothed, PROFILE_FLOATS, PROFILE_ROWS, PROFILES, profileTable, RAIL_CODE, RECORD, RIVER, ROAD, RULES, SEA, seaShows, SHOULDER_M, shows, SLEEPER_M, STREAM, surfaceOf, SURFACE_CODES, SURFACES, AREA, TUNNEL, WARP_M, waterAt, type Line, type Profile } from './bins';
import { cornersOf, halfAt, partnersOf } from './corners';
import { LEFT, RIGHT } from './town';

const shader = readFileSync(new URL('./stamp-core.wgsl', import.meta.url), 'utf8');
const fixture = (z: number, x: number, y: number) =>
    decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/${z}/${x}/${y}.bin`, import.meta.url)))));
/** Trübsee: the zoom-13 tile, and the level-17 node over its middle. */
const TILE = { level: 13, x: 4286, y: 2888 };
/** The entries of cell c of a node's stamp data, and the origin of its lake crossings, as stamp-core.wgsl reads them. */
function cellOf(data: Uint32Array, node: { level: number; x: number; y: number }, c: number) {
    const head = 2 + GRID * GRID + 1, offset = data[2 + c], shift = 22 - node.level, cell = CELL * 2 ** shift;
    const entries = data.subarray(head + (offset & 0xffffff), head + (data[2 + c + 1] & 0xffffff));
    const held = (v: number, first: number) => Math.min(Math.max(v, first * 2 ** shift), (first + PAGE) * 2 ** shift - 1);
    const origin = [held((node.x * PAGE - CELL) * 2 ** shift + (c % GRID) * cell + cell / 2, node.x * PAGE) + ((offset >>> 24) & 15) - 8,
        held((node.y * PAGE - CELL) * 2 ** shift + Math.floor(c / GRID) * cell + cell / 2, node.y * PAGE) + (offset >>> 28) - 8];
    return { entries, origin };
}

/** The gates of stamp-core.wgsl in JavaScript. */
const shaderGates = () => shaderFunctions<Record<'road_shows' | 'path_shows' | 'sea_shows', (h: number, ...rest: unknown[]) => boolean>>(shader, ['reach_limit', 'gate_texel', 'road_shows', 'path_shows', 'water_lift', 'sea_shows']);

const lakeNode = () => {
    const { rings } = features(fixture(TILE.level, TILE.x, TILE.y), TILE).lakes[0], xs = rings[0].filter((_, i) => i % 2 === 0), ys = rings[0].filter((_, i) => i % 2 === 1);
    const middle = (values: number[]) => (Math.min(...values) + Math.max(...values)) / 2;
    return { level: 17, x: Math.floor(middle(xs) / 2 ** 13), y: Math.floor(middle(ys) / 2 ** 13) };
};

describe('the stamp data of a node', () => {
    it('gates the lines with the numbers of stamp-core.wgsl, packs the profiles of numbers of 0 or more in the order of its struct, and numbers the surfaces as the shaders do', () => {
        for (const [name, value] of Object.entries(RULES)) expect(Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(shader)?.[1]), name).toBe(value);
        const fields = /struct Profile \{([^}]*)\}/.exec(shader)![1].match(/\w+(?=:)/g)! as (keyof Profile)[];
        const rows = Object.values(PROFILES), table = profileTable();
        expect(Number(/const PROFILE_ROWS: u32 = (\d+)u;/.exec(shader)?.[1])).toBe(PROFILE_ROWS);
        expect(Number(/const WARP_M: f32 = ([\d.]+);/.exec(shader)?.[1])).toBe(WARP_M);
        expect(Number(/const MASK_LEVEL: u32 = (\d+)u;/.exec(readFileSync(new URL('./features.wgsl', import.meta.url), 'utf8'))?.[1])).toBe(NEAR);
        rows.forEach((row, i) => expect(Array.from(table.subarray(PROFILE_FLOATS * i, PROFILE_FLOATS * i + fields.length))).toEqual(fields.map(field => Math.fround(row[field]))));
        // The shoulders follow the rows, one per surface code, as `Rules` of stamp-core.wgsl has them.
        expect(Number(/shoulders: array<vec4f, (\d+)>/.exec(shader)?.[1]) * 4).toBe(SURFACE_CODES);
        expect(table.length).toBe(PROFILE_ROWS * PROFILE_FLOATS + SURFACE_CODES);
        // The bench holds more than a texel of each level that draws it, so the bilinear heights at a path's edge stay on it, and it ends before the reach.
        const bench = Number(/const BENCH_TEXELS: f32 = ([\d.]+);/.exec(shader)?.[1]), { bench_metres: metres } = shaderFunctions<{ bench_metres: (texels: number, reach: number, texel: number) => number }>(shader, ['bench_metres']);
        for (const level of [18, 17, 16]) expect(metres(bench, PROFILES.path.reach, gateTexel(level))).toBeGreaterThan(gateTexel(level));
        for (const level of [15, 12]) expect(metres(bench, PROFILES.path.reach, gateTexel(level))).toBeLessThan(PROFILES.path.reach);
        for (const [surface, code] of [['main', 1], ['grass', 5], ['path', 6], ['oneway', 7]] as const) expect(table[PROFILE_ROWS * PROFILE_FLOATS + code]).toBeCloseTo(SHOULDER_M[surface] ?? 0, 6);
        for (const [name, row] of Object.entries(PROFILES)) for (const field of fields) expect(row[field], `${name}.${field}`).toBeGreaterThanOrEqual(0);
        // The shaders know the surfaces by their codes, and the mask holds a code in 4 bits.
        expect(Object.keys(SURFACES)).toEqual(['main', 'road', 'gravel', 'dirt', 'grass', 'path', 'oneway', 'paving', 'rail', 'narrow', 'rack', 'narrow-rack']);
        const mask = readFileSync(new URL('./features.wgsl', import.meta.url), 'utf8');
        expect([/const TRACK_CODE: u32 = (\d+)u;/.exec(shader)?.[1], /const RAIL_CODE: u32 = (\d+)u;/.exec(mask)?.[1]].map(Number)).toEqual([RAIL_CODE, RAIL_CODE]);
        expect(Number(/const PAVING_CODE: u32 = (\d+)u;/.exec(mask)?.[1])).toBe(Object.keys(SURFACES).indexOf('paving') + 1);
        expect([/const SLEEPER_M: f32 = ([\d.]+);/.exec(shader)?.[1], /const SLEEPER_SPACING_M: f32 = ([\d.]+);/.exec(mask)?.[1]].map(Number)).toEqual([SLEEPER_M, SLEEPER_M]);
        const stamp = readFileSync(new URL('./stamp.wgsl', import.meta.url), 'utf8'), value = (source: string, name: string) => Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(source)?.[1]);
        expect(['TRACK_M', 'MASK_M', 'MASK_KNEE'].map(name => value(mask, name))).toEqual(['TRACK_RANGE', 'MASK_RANGE', 'MASK_KNEE'].map(name => value(stamp, name)));
        const [rail, narrow] = ['RAIL_HALF_M', 'NARROW_HALF_M'].map(name => Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(mask)?.[1]));
        expect(Object.values(SURFACES).slice(RAIL_CODE - 1)).toEqual([rail, narrow, rail, narrow]);
    });

    it('holds the tangent, the bank and the widening of an alignment in each record of a line', () => {
        const tile = { level: 13, x: 4287, y: 2889 }, tangent = Math.PI / 4, xy = [4000, 4000, 4400, 4400].map(v => v * 4);
        const plan = { along: 0, backward: false, tangent: Float32Array.of(tangent, tangent), grade: new Float32Array(2), bank: Float32Array.of(0.05, 0.05), widen: Float32Array.of(0.5, 0.5), side: new Float32Array(2) };
        const list = [{ kind: 'road', tags: { highway: 'tertiary' }, geometry: 'line' as const, parts: [{ outer: false, xy: Float32Array.from(xy, v => v / 4), z: Float32Array.of(1000, 1000), plan }] }];
        const [line] = features(list, tile).lines, shift = 22 - 17, node = { level: 17, x: Math.floor(line.xy[0] / (PAGE * 2 ** shift)), y: Math.floor(line.xy[1] / (PAGE * 2 ** shift)) };
        const { data } = nodeStamps(list, tile, node);
        const records = 2 + GRID * GRID + 1 + data![1], word = (i: number) => data![records + RECORD * (data![0] - 1) + i];
        const snorm = (w: number) => [(w << 16) >> 16, w >> 16].map(v => v / 32767);
        expect(word(4)).toBe(ROAD);
        for (const end of [9, 10]) for (const value of snorm(word(end) | 0)) expect(value).toBeCloseTo(Math.SQRT1_2, 3);
        // The rise to the right: x runs east and y south, so the left of the line is its higher side.
        for (const value of snorm(word(11) | 0)) expect(value).toBeCloseTo(-0.05, 3);
        expect([(word(13) << 8) >> 24, (word(13) | 0) >> 24]).toEqual([25, 25]);
        const unit = metresPerUnit(line.xy[1]), left = [line.xy[0] + (100 + 2 * Math.SQRT1_2 / unit), line.xy[1] + (100 - 2 * Math.SQRT1_2 / unit)] as [number, number];
        expect(roadHeight(line, left, 3 / unit, unit)).toBeCloseTo(1000.1, 3);
    });

    it('reads the mitres and the bank in the words that a record packs them in', () => {
        expect([/let ma = unpack2x16snorm\(word\(r \+ (\d+)u\)\)/, /let mb = unpack2x16snorm\(word\(r \+ (\d+)u\)\)/, /let bank = unpack2x16snorm\(word\(r \+ (\d+)u\)\)/].map(word => Number(word.exec(shader)?.[1]))).toEqual([9, 10, 11]);
    });

    it('digs a ditch beside a cut, only on the level with two texels in it, and starts the slope past its bottom', () => {
        const { ditch_depth: depth, ditch_shows: shows, cut_top: top, widen } = shaderFunctions<{ ditch_depth: (x: number, depth: number) => number; ditch_shows: (level: number) => boolean; cut_top: (x: number, reach: number, slope: number, ditch: number) => number; widen: (x: number, reach: number, slope: number) => number }>(shader, ['ditch_depth', 'ditch_shows', 'gate_texel', 'cut_top', 'widen']);
        const end = Number(/const DITCH_END_M: f32 = ([\d.]+);/.exec(shader)?.[1]);
        expect([depth(0, 0.3), depth(0.4, 0.3), depth(0.7, 0.3), depth(end, 0.3)]).toEqual([0, 0.3, 0.3, 0]);
        expect([18, 17, 16, 14].map(shows)).toEqual([true, false, false, false]);
        // Without a ditch the cut rises from the core; with one the ground lies under the road at its bottom, and rises from there.
        for (const x of [0, 0.5, 2]) expect(top(x, 10, 0.85, 0)).toBe(widen(x, 10, 0.85));
        expect(top(0, 10, 0.85, 0.3)).toBe(0);
        expect(top(0.6, 10, 0.85, 0.3)).toBeCloseTo(-0.3, 9);
        expect(top(2, 10, 0.85, 0.3)).toBeCloseTo(widen(2 - 0.7, 10, 0.85), 9);
        // The stamp holds its cuts by these, and only a plain road takes a ditch.
        const body = functionOf(shader, 'stamp_at');
        expect(body).toMatch(/let ditch = select\(0\.0, rule\.ditch, plain && ditch_shows\(level\)\);/);
        expect(body).toMatch(/let u = g\.z \+ tol \+ cut_top\(max\(0\.0, min\(x, 2\.0 \* \(x - shelf\)\)\), reach, select\(rule\.cut, GALLERY_VALLEY_CUT, gallery\), ditch\);/);
    });

    it('rounds the top of a cut and the foot of a fill far from the road, and keeps the crease beside it', () => {
        const stamp = readFileSync(new URL('./stamp.wgsl', import.meta.url), 'utf8');
        const { rounded } = shaderFunctions<{ rounded: (base: number, s: object) => number }>(stamp, ['rounded']);
        const cut = (x: number) => ({ cut: true, hi: 10, cut_x: x, fill: false }), fill = (x: number) => ({ fill: true, lo: 10, fill_x: x, cut: false });
        // Beside the road the slope meets the ground in a crease: the bound and nothing else.
        expect([rounded(12, cut(1)), rounded(9, cut(1)), rounded(8, fill(1)), rounded(11, fill(1))]).toEqual([10, 9, 10, 11]);
        // Past SOFT_FULL_M it rounds over DAYLIGHT_M: a fifth of a metre at the crease, nothing at the ends of the arc.
        expect(rounded(10, cut(5))).toBeCloseTo(9.8, 6);
        expect(rounded(10, fill(5))).toBeCloseTo(10.2, 6);
        expect([rounded(10.8, cut(5)), rounded(9.2, fill(5))]).toEqual([10, 10]);
        // It never falls as the base rises.
        for (let base = 8; base < 12; base += 0.05) expect(rounded(base + 0.05, cut(5))).toBeGreaterThanOrEqual(rounded(base, cut(5)) - 1e-9);
        // The stamp pass takes the ground from it, then clamps.
        expect(functionOf(stamp, 'stamp')).toMatch(/h = select\(NONE, clamp\(rounded\(base, s\), s\.lo, s\.hi\), known\(base\)\);/);
    });

    it('marks a shoulder in the byte of a sidewalk, and the paint and the snap each read the part that is theirs', () => {
        const source = readFileSync(new URL('./mask-walk.wgsl', import.meta.url), 'utf8');
        const { walk_byte: pack, walk_metres: metres, holds_shoulder: holds, is_trodden: trodden, is_marked: lined } = shaderFunctions<{ walk_byte: (walk: number, shoulder: boolean, trodden: boolean, marked: boolean) => number; walk_metres: (byte: number) => number; holds_shoulder: (byte: number) => boolean; is_trodden: (byte: number) => boolean; is_marked: (byte: number) => boolean }>(source, ['walk_byte', 'walk_metres', 'holds_shoulder', 'is_trodden', 'is_marked']);
        for (const walk of [...Object.values(PROFILES).map(({ walk }) => walk), 0]) for (const shoulder of [false, true]) for (const worn of [false, true]) for (const lines of [false, true]) {
            const byte = pack(walk, shoulder, worn, lines);
            expect(byte).toBeLessThan(256);
            expect(metres(byte)).toBeCloseTo(walk, 6);
            expect(holds(byte)).toBe(shoulder);
            expect(trodden(byte)).toBe(worn);
            expect(lined(byte)).toBe(lines);
        }
        const stamp = readFileSync(new URL('./stamp.wgsl', import.meta.url), 'utf8'), mask = readFileSync(new URL('./features.wgsl', import.meta.url), 'utf8');
        expect(functionOf(stamp, 'stamp')).toContain('walk_byte(s.walk, s.shoulder, s.trodden, s.marked)');
        expect(functionOf(stamp, 'snap')).toMatch(/walk = max\(walk, walk_metres\(wide\)\);\s+shoulder = max\(shoulder, select\(0\.0, shoulder_of\(surface\), holds_shoulder\(wide\)\)\);/);
        expect(functionOf(mask, 'mask_at')).toMatch(/walk_metres\(past\), is_trodden\(past\), is_marked\(past\)/);
        // Only a plain road marks it, and a corner's arc takes it away.
        const body = functionOf(shader, 'stamp_at');
        expect(body).toContain('let plain = kind == ROAD && walk == 0.0 && !gallery && road_shows(own, rule, level);');
        expect(body).toContain('out.shoulder = plain;');
        expect(body.match(/out\.shoulder = false;/g)).toHaveLength(2);
    });

    it('shows a line on the levels where the gates of stamp-core.wgsl show it', () => {
        const gates = shaderGates(), seen = new Set<boolean>();
        for (const [kind, profile] of [[ROAD, 0], [PATH, Object.keys(PROFILES).indexOf('path')], [RIVER, Object.keys(PROFILES).indexOf('river')]]) for (const h of [0.6, 2, 5, 12]) for (let level = 12; level <= 18; level++) {
            const line = { kind, h, profile } as Line, rule = Object.values(PROFILES)[profile];
            const expected = (kind === PATH ? gates.path_shows : gates.road_shows)(h, rule, level);
            expect(shows(line, gateTexel(level)), `kind ${kind}, h ${h}, level ${level}`).toBe(expected);
            expect(shows({ ...line, kind: STREAM }, gateTexel(level))).toBe(false);
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
            // Junctions, then each lake's edges and its marker, then the corners, and the segments, each line's together.
            const kinds = Array.from(entries, entry => `${word(entry & ~INSIDE, 4)},`).join('');
            expect(kinds).toMatch(new RegExp(`^(${JUNCTION},)*((${EDGE},)*${LAKE},)*(${CORNER},)*((${ROAD}|${PATH}),)*$`));
            const lines = Array.from(entries, entry => entry & ~INSIDE).filter(r => [ROAD, PATH].includes(word(r, 4))).map(r => word(r, 13) & 0xffff);
            expect(lines.filter((id, i) => id !== lines[i - 1]).length).toBe(new Set(lines).size);
            const marker = Array.from(entries).find(entry => word(entry & ~INSIDE, 4) === LAKE);
            if (insideRings(lake.rings, origin[0], origin[1])) {
                inside++;
                expect(marker! & INSIDE).toBe(INSIDE);
            }
        }
        expect(inside).toBeGreaterThan(0);
        // A node inside the lake holds a mask, which takes the lake's inside, where nothing grows.
        const middle = { level: 18, x: 2 * NODE.x + 1, y: 2 * NODE.y + 1 }, held = nodeStamps(list, TILE, middle);
        expect(cellOf(held.data!, middle, 9 * GRID + 9).entries.length).toBe(1);
        expect(held.mask).toBe(true);
    });

    it('takes the points inside the buildings and the river areas, from each cell origin and the edges that the cell lists', () => {
        // Engelberg: a level-17 node on the river through the village.
        const tile = { level: 13, x: 4287, y: 2887 }, list = fixture(tile.level, tile.x, tile.y), { areas, buildings } = features(list, tile);
        const [x, y] = areas[0].rings[0], NODE = { level: 17, x: Math.floor(x / 2 ** 13), y: Math.floor(y / 2 ** 13) };
        const { data } = nodeStamps(list, tile, NODE);
        const records = 2 + GRID * GRID + 1 + data![1], word = (r: number, i: number) => data![records + RECORD * r + i] | 0;
        const shift = 22 - NODE.level, cell = CELL * 2 ** shift, x0 = NODE.x * PAGE * 2 ** shift - cell, y0 = NODE.y * PAGE * 2 ** shift - cell;
        const seen = new Set<boolean>();
        for (let j = 1; j < GRID - 1; j++) for (let i = 1; i < GRID - 1; i++) {
            const { entries, origin } = cellOf(data!, NODE, j * GRID + i);
            for (let v = 0; v < 4; v++) for (let u = 0; u < 4; u++) {
                const p = [x0 + (i + (u + 0.3) / 4) * cell, y0 + (j + (v + 0.3) / 4) * cell];
                // As stamp_at: an outline holds p where the edges before its marker cross the way from the origin an odd number of times, or none where the origin is inside.
                let crossed = false, taken = false;
                for (const entry of entries) {
                    const r = entry & ~INSIDE, kind = word(r, 4);
                    if (kind === EDGE) crossed = crossed !== crosses(origin, p, [0, 1, 2, 3].map(k => word(r, k)));
                    if (kind === AREA) taken ||= ((entry & INSIDE) !== 0) !== crossed;
                    if (kind !== EDGE) crossed = false;
                }
                expect(taken, `${p}`).toBe([...areas, ...buildings].some(({ rings }) => insideRings(rings, p[0], p[1])));
                seen.add(taken);
            }
        }
        expect(seen.size).toBe(2);
    });

    it('cuts the sea on the levels where its lift does not reach its bed, as stamp-core.wgsl gates it', () => {
        const gates = shaderGates(), seen = new Set<boolean>();
        for (let level = 8; level <= 18; level++) {
            expect(seaShows(level), `level ${level}`).toBe(gates.sea_shows(level));
            seen.add(seaShows(level));
        }
        expect(seen.size).toBe(2);
    });

    it('holds the ground in the sea under its bed, sloping from a shore band inside the coast, and leaves the land and the islands as they are', () => {
        // The western half of a level-17 node is sea, with an island of 100 tile units; the node four nodes east is land, the node four west open sea.
        const NODE = { level: 17, x: TILE.x * 16 + 8, y: TILE.y * 16 + 8 }, coast = 8704, ring = (x0: number, y0: number, x1: number, y1: number) => Int32Array.from([x0, y0, x1, y0, x1, y1, x0, y1]);
        const list = [{ kind: 'sea', tags: {}, geometry: 'area' as const, parts: [
            { outer: true, xy: ring(-100, -100, coast, EXTENT + 100), z: new Float32Array(4) },
            { outer: false, xy: ring(8300, 8600, 8400, 8700), z: new Float32Array(4) },
        ] }];
        const { data, heights, mask } = nodeStamps(list, TILE, NODE);
        expect([heights, mask]).toEqual([true, true]);
        expect(nodeStamps(list, TILE, { ...NODE, x: NODE.x + 4 }).heights).toBe(false);
        const open = nodeStamps(list, TILE, { ...NODE, x: NODE.x - 4 });
        expect([open.heights, open.mask]).toEqual([true, false]);
        const scale = unitScale(TILE.level), point = (tx: number, ty: number) => [(TILE.x * EXTENT + tx) * scale, (TILE.y * EXTENT + ty) * scale];
        const bound = (tx: number, ty: number) => seaBound(data!, NODE, point(tx, ty));
        // Metres per tile unit, and the shore band of the node's level.
        const unit = metresPerUnit(point(coast, 8900)[1]), metres = scale * unit, band = RULES.SHORE_CELLS * RULES.MESH_TEXELS * unit * 2 ** (22 - NODE.level);
        expect(bound(8250, 8900)).toBe(-RULES.BED_DEPTH);
        expect(bound(coast - band / 2 / metres, 8900)).toBeCloseTo(0, 6);
        expect(bound(coast - (band + 2) / metres, 8900)).toBeCloseTo(-RULES.SHORE_SLOPE * 2, 2);
        for (const [x, y] of [[coast + 1, 8900], [coast + 0.5 / metres, 8300], [9100, 8250], [8350, 8650], [8301, 8601]]) expect(bound(x, y), `${x}, ${y}`).toBe(Infinity);
        // The eye stands on the sea's level, and on the ground of the land and the island.
        expect([[8250, 8900], [9100, 8250], [8350, 8650]].map(([x, y]) => waterAt(data!, NODE, ...(point(x, y) as [number, number])))).toEqual([0, -Infinity, -Infinity]);
        // A level whose lift passes the bed does not cut the sea.
        const coarse = { level: 11, x: TILE.x >> 2, y: TILE.y >> 2 };
        expect(seaShows(coarse.level)).toBe(false);
        expect(nodeStamps(list, TILE, coarse).heights).toBe(false);
    });

    it('takes the side of the sea for every texel of a node at a tile edge from the coast inside the buffer, not from the joins of the ring past it', () => {
        // A bay enters the tile from the east. The tile's ring joins its vertices past the buffer straight
        // across the bay, inside the apron of the level-14 node at the tile's east edge.
        const NODE = { level: 14, x: TILE.x * 2 + 1, y: TILE.y * 2 }, join = EXTENT + 420;
        const list = [{ kind: 'sea', tags: {}, geometry: 'area' as const, parts: [{ outer: true, xy: Int32Array.from([16000, 4000, join, 4000, join, 12000, 16000, 12000]), z: new Float32Array(4) }] }];
        const { data } = nodeStamps(list, TILE, NODE), scale = unitScale(TILE.level);
        expect(metresPerUnit(TILE.y * EXTENT * scale) * scale * 420).toBeGreaterThan(RULES.BUFFER_M);
        for (let ty = 64; ty < 8192; ty += 128) for (let tx = 8192 + 32; tx < EXTENT; tx += 128) {
            const p = [(TILE.x * EXTENT + tx) * scale, (TILE.y * EXTENT + ty) * scale], sea = tx > 16000 && ty > 4000;
            expect(seaBound(data!, NODE, p) < Infinity, `${tx}, ${ty}`).toBe(sea);
            expect(waterAt(data!, NODE, p[0], p[1])).toBe(sea ? 0 : -Infinity);
        }
    });
});

describe('the audit of the sea', () => {
    it('samples the bed over a tunnel in the sea, where a cover that lifted the bed would show', () => {
        // Sea over the whole tile, and a road tunnel 2 m under sea level across the level-17 node in its middle.
        const NODE = { level: 17, x: TILE.x * 16 + 8, y: TILE.y * 16 + 8 }, scale = unitScale(TILE.level);
        const list: Feature[] = [
            { kind: 'sea', tags: {}, geometry: 'area' as const, parts: [{ outer: true, xy: Int32Array.from([-100, -100, EXTENT + 100, -100, EXTENT + 100, EXTENT + 100, -100, EXTENT + 100]), z: new Float32Array(4) }] },
            { kind: 'tunnel', tags: { highway: 'tertiary', tunnel: 'yes' }, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from([8000, 8704, 9400, 8704]), z: Float32Array.from([-2, -2]) }] },
        ];
        const samples = auditSamples(list, TILE, NODE), floats = new Float32Array(samples.buffer), y = (TILE.y * EXTENT + 8704) * scale;
        const metres = metresPerUnit(y);
        const over = Array.from({ length: samples.length / SAMPLE }, (_, i) => i * SAMPLE)
            .filter(at => samples[at + 2] === (IN_WATER | (SEA << 8)) && Math.abs(samples[at + 1] - y) * metres < 4);
        expect(over.length).toBeGreaterThan(0);
        // Each holds the sea's level, 0 m, and a distance from the coast past the start of the bed's slope.
        for (const at of over) expect([floats[at + 3], floats[at + 5] > 0]).toEqual([0, true]);
    });
});

/** As stamp_at: the upper bound that the sea puts on the ground at world point p of a node, Infinity where it holds none. */
function seaBound(data: Uint32Array, node: { level: number; x: number; y: number }, p: number[]): number {
    const records = 2 + GRID * GRID + 1 + data[1], word = (r: number, i: number) => data[records + RECORD * r + i] | 0;
    const unit = metresPerUnit(p[1]), texel = unit * 2 ** (22 - node.level);
    const band = Math.max(0, Math.min(RULES.SHORE_CELLS * RULES.MESH_TEXELS * texel, RULES.BUFFER_M - 2 * texel - 1 - RULES.BED_DEPTH / RULES.SHORE_SLOPE));
    const { entries, origin } = cellAt(data, node, p[0], p[1]);
    let crossed = false, shore = Infinity, hi = Infinity;
    for (const entry of entries) {
        const r = entry & ~INSIDE, kind = word(r, 4), [ax, ay, bx, by] = [0, 1, 2, 3].map(k => word(r, k));
        if (kind === EDGE) {
            crossed = crossed !== crosses(origin, p, [ax, ay, bx, by]);
            shore = Math.min(shore, ringsDistance([[ax, ay, bx, by]], p[0], p[1]) * unit);
            continue;
        }
        if (kind === SEA && ((entry & INSIDE) !== 0) !== crossed) hi = Math.min(hi, -Math.min(RULES.BED_DEPTH, RULES.SHORE_SLOPE * Math.max(0, shore - band)));
        crossed = false;
        shore = Infinity;
    }
    return hi;
}

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

describe('the sleepers of a track', () => {
    it('are whole at each OSM vertex, so two tiles that cut the track elsewhere paint the same ones', () => {
        const xy = [0, 0, 400, 0, 800, 300, 900, 800, 1300, 1000, 1700, 900, 2000, 400];
        const track = (from: number, to: number) => features([{ kind: 'railway', tags: { railway: 'rail' }, geometry: 'line', parts: [{
            outer: false, xy: Int32Array.from(xy.slice(2 * from, 2 * to + 2)), z: new Float32Array(to - from + 1).fill(1000),
        }] }], TILE).lines[0];
        const fractions = (line: Line, from: number, to: number) => {
            const osm = line.anchors!.flatMap((anchor, v) => (anchor ? [v] : []));
            return line.sleepers!.slice(osm[from], osm[to] + 1).map(count => count - Math.floor(count + 1e-6));
        };
        const whole = track(0, 6), a = track(0, 4), b = track(2, 6);
        for (const line of [whole, a, b]) for (const fraction of fractions(line, 0, line.anchors!.filter(Boolean).length - 1).filter((_, v, all) => v === 0 || v === all.length - 1)) expect(fraction).toBeCloseTo(0, 4);
        expect(whole.sleepers!.at(-1)! / SLEEPER_M).toBeGreaterThan(0);
        // Tile A rounds segment 2-3 as the whole line does, and tile B segment 3-4.
        fractions(a, 2, 3).forEach((fraction, i) => expect(fraction).toBeCloseTo(fractions(whole, 2, 3)[i], 4));
        fractions(b, 1, 2).forEach((fraction, i) => expect(fraction).toBeCloseTo(fractions(whole, 3, 4)[i], 4));
    });
});

describe('a junction', () => {
    const tile = { level: 13, x: 4287, y: 2889 }, unit = metresPerUnit((tile.y * EXTENT + 4000) * unitScale(tile.level));
    it('holds a road\'s round core and paint past a dead end only: a path that runs on leaves a plain end', () => {
        const road = way('road', { highway: 'residential' }, [1000, 4000, 3000, 4000], [1000, 1000]), path = way('path', { highway: 'path' }, [3000, 4000, 5000, 4000], [1000, 1000]);
        /** The flags of the last segment of the road, from a level-18 node about its end. */
        const flags = (list: Feature[]) => {
            const [x, y] = features(list, tile).lines[0].xy.slice(2), node = { level: 18, x: Math.floor(x / 2 ** 12), y: Math.floor(y / 2 ** 12) }, { data } = nodeStamps(list, tile, node);
            const start = 2 + GRID * GRID + 1 + data![1];
            for (let r = start; r < data!.length; r += RECORD) if (data![r + 4] === ROAD) return data![r + 5] >>> 8 & 255;
            throw new Error('no road');
        };
        const CORE_B = 4;
        expect(flags([road]) & CORE_B).toBe(CORE_B);
        expect(flags([road, path]) & CORE_B).toBe(0);
    });

    /** A way with no alignment; with `side`, the alignment of a straight way that marks those ways out as sides. */
    const way = (kind: string, tags: Record<string, string>, xy: number[], z: number[], side?: number[]) => {
        const column = (value: number) => Float32Array.from(z, () => value), heading = Math.atan2(xy[3] - xy[1], xy[2] - xy[0]);
        const plan = side && { along: 0, backward: false, tangent: column(heading), grade: column(0), bank: column(0), widen: column(0), side: Float32Array.from(side) };
        return { kind, tags, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from(xy), z: Float32Array.from(z), plan }] };
    };

    it('tilts a side road across its mouth to the surface of the through road, and not past the fade', () => {
        // A secondary road climbs east at 8 % through a junction; a residential road leaves it to the north.
        const metres = unitScale(tile.level) * unit, at = (x: number) => 1000 + 0.08 * (x - 3000) * metres;
        const { lines } = features([
            way('main-road', { highway: 'secondary' }, [1000, 4000, 3000, 4000, 5000, 4000], [at(1000), at(3000), at(5000)]),
            // Its vertex at the approach distance of the solver, 20 units (4.1 m) from the junction.
            way('road', { highway: 'residential' }, [3000, 4000, 3000, 3980, 3000, 2000], [at(3000), at(3000) + 0.1, at(3000) + 20], [SIDE_ON, 0, 0]),
        ], tile);
        const side = lines.find(line => line.profile === Object.keys(PROFILES).indexOf('residential'))!;
        // 2 m east of the side road's middle and 1 m north of the junction, inside the through road.
        const p: [number, number] = [side.xy[0] + 2 / unit, side.xy[1] - 1 / unit];
        expect(roadHeight(side, p, 3 / unit, unit)).toBeCloseTo(at(3000) + 0.08 * 2 + 0.1 / (20 * metres), 2);
        // 50 m along, on the long segment that a tile without the junction stamps too: no tilt.
        const far = (east: number): [number, number] => [side.xy[0] + east / unit, side.xy[1] - 50 / unit];
        expect(roadHeight(side, far(2), 3 / unit, unit)).toBeCloseTo(roadHeight(side, far(0), 3 / unit, unit), 6);
    });

    it('tilts a side road to the banked surface of the through road', () => {
        // A level secondary road banks east along y = 4000, its left to the north.
        const banked = (xy: number[]) => {
            const road = way('main-road', { highway: 'secondary' }, xy, [1000, 1000], [0, 0]);
            road.parts[0].plan!.bank.fill(0.05);
            return road;
        };
        const { lines } = features([banked([1000, 4000, 3000, 4000]), banked([3000, 4000, 5000, 4000]), way('road', { highway: 'residential' }, [3000, 4000, 4000, 3000], [1000, 1000], [SIDE_ON, 0])], tile);
        const side = lines.find(line => line.profile === Object.keys(PROFILES).indexOf('residential'))!;
        // The side road runs north-east: its right side lies south-east, 0.05 × √½ lower per metre.
        const p: [number, number] = [side.xy[0] + (Math.SQRT2 + Math.SQRT1_2) / unit, side.xy[1] + (Math.SQRT1_2 - Math.SQRT2) / unit];
        expect(crossSlope(side, 0, 2, 100)).toBeCloseTo(-0.05 * Math.SQRT1_2, 3);
        expect(roadHeight(side, p, 3 / unit, unit)).toBeCloseTo(1000 - 0.05 * Math.SQRT1_2, 3);
    });

    it('runs a road on onto a bridge and not into the side road at the end of the bridge, and tapers it there', () => {
        const flat = [1000, 1000], road = { highway: 'secondary', width: '6' };
        const { lines } = features([
            way('main-road', road, [1000, 4000, 3000, 4000], flat),
            way('main-road', { ...road, bridge: 'yes', width: '8' }, [3000, 4000, 5000, 4000], flat),
            way('road', { highway: 'service' }, [3000, 4000, 3000, 2000], flat, [SIDE_ON, 0]),
        ], tile);
        const [main, bridge, service] = lines, partners = partnersOf(lines);
        expect(bridge.kind).toBe(BRIDGE);
        expect(partners.get(main)).toEqual([undefined, bridge]);
        expect(partners.has(service)).toBe(false);
        expect(halfAt(main, 1, partners)).toBeCloseTo((main.h + bridge.h) / 2, 6);
        expect(main.join).toEqual(undefined);
    });

    it('takes two ways of two classes as one road, and a way that passes through has no partner', () => {
        const flat = [1000, 1000];
        const split = features([way('road', { highway: 'tertiary' }, [1000, 4000, 3000, 4000], flat), way('road', { highway: 'service' }, [3000, 4000, 5000, 4100], flat)], tile).lines;
        expect(partnersOf(split).get(split[1])).toEqual([split[0], undefined]);
        // A tertiary passes through the vertex where a service road ends.
        const passes = features([
            way('road', { highway: 'tertiary' }, [1000, 4000, 3000, 4000, 5000, 4000], [1000, 1000, 1000]),
            way('road', { highway: 'service' }, [3000, 4000, 3000, 2000], flat, [SIDE_ON, 0]),
        ], tile).lines;
        expect(partnersOf(passes).size).toBe(0);
    });

    it('puts a road at a level crossing on the plane of the track, and the road does not run on across it', () => {
        // A track climbs north at 5 % across a road that OSM splits at the crossing.
        const metres = unitScale(tile.level) * unit, at = (y: number) => 1000 + 0.05 * (4000 - y) * metres, flat = [at(4000), at(4000)], road = { highway: 'tertiary' };
        const lines = features([
            way('road', road, [1000, 4000, 3000, 4000], flat, [0, SIDE_BACK]),
            way('road', road, [3000, 4000, 5000, 4000], flat, [SIDE_ON, 0]),
            way('railway', { railway: 'rail' }, [3000, 2000, 3000, 4000, 3000, 6000], [at(2000), at(4000), at(6000)]),
        ], tile).lines;
        expect(partnersOf(lines).size).toBe(0);
        expect(Math.abs(crossSlope(lines[1], 0, 0, 100))).toBeCloseTo(0.05, 3);
        expect(Math.abs(crossSlope(lines[0], 0, 100, 0))).toBeCloseTo(0.05, 3);
    });

    it('keeps the marks of a line past a vertex that stands alone between unknown heights', () => {
        const xy = [1000, 4000, 1500, 4000, 2000, 4000, 2500, 4000, 3000, 4000, 3500, 4000, 4000, 4000];
        const { lines } = features([way('road', { highway: 'service' }, xy, [1000, NaN, 1000, NaN, 1000, 1000, 1000], [0, 0, 0, 0, 0, SIDE_ON, 0])], tile);
        expect(lines.map(line => line.sideWays)).toEqual([[0, SIDE_ON, 0]]);
    });

    it('marks the way out of the visit that the bake marks, where a way passes a point twice', () => {
        // A stem runs into a loop that ends on the stem's last vertex, as a side of the junction.
        const xy = [1000, 4000, 3000, 4000, 3500, 3500, 4000, 4000, 3500, 4500, 3000, 4000];
        const { lines } = features([way('road', { highway: 'service' }, xy, [1000, 1000, 1000, 1000, 1000, 1000], [0, 0, 0, 0, 0, SIDE_BACK])], tile);
        const [ways] = junctionWays(lines), last = lines[0].z.length - 1;
        expect(ways.map(({ v, step, side }) => [v === last ? 'end' : 'pass', step, side]).sort()).toEqual([['end', -1, true], ['pass', -1, false], ['pass', 1, false]]);
    });
});

describe('the ends of bridges and tunnels', () => {
    it('meet the ground at a road on it and where a bridge runs into a tunnel, not where a tunnel runs on', () => {
        const way = (kind: string, tags: Record<string, string>, x0: number, x1: number) =>
            ({ kind, tags: { highway: 'motorway', ...tags }, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from([x0, 4000, x1, 4000]), z: Float32Array.from([500, 500]) }] });
        const { lines } = features([
            way('main-road', {}, 1000, 2000),
            way('main-road', { bridge: 'yes' }, 2000, 3000),
            way('tunnel', { tunnel: 'yes' }, 3000, 4000),
            way('tunnel', { tunnel: 'yes' }, 4000, 5000),
        ], { level: 14, x: 8574, y: 5778 });
        expect(lines.filter(line => line.kind === BRIDGE || line.kind === TUNNEL).map(line => line.abut)).toEqual([[true, true], [true, false], [false, false]]);
    });
});

describe('a path', () => {
    it('without a `width` tag is on its widest surface as wide as the bake takes it, and a pedestrian bridge is a path', () => {
        const rust = readFileSync(new URL('../../../../../../host/obc-trailview-features/src/structures.rs', import.meta.url), 'utf8');
        const block = /^const PATH_HALF_M: [^=]+= \[([^\]]*)\];$/m.exec(rust)![1];
        const table: Record<string, number> = Object.fromEntries([...block.matchAll(/\("([a-z_]+)", ([0-9.]+)\)/g)].map(([, highway, half]) => [highway, Number(half)]));
        expect(Object.keys(table).length).toBeGreaterThan(5);
        for (const [highway, half] of Object.entries(table)) {
            const widest = Math.max(...['', 'asphalt', 'gravel', 'dirt', 'grass'].map(surface => halfWidth({ highway, surface }, surfaceOf({ highway, surface }))));
            expect(widest, highway).toBe(half);
            expect(isPath({ profile: Object.keys(PROFILES).indexOf(highway) } as Line), highway).toBe(true);
        }
        expect(isPath({ profile: Object.keys(PROFILES).indexOf('track') } as Line)).toBe(false);
    });
});

describe('a town', () => {
    // A residential road runs east; a house of 10 × 8 m stands south of it, on its right, `gap` metres from its edge.
    const tile = { level: 13, x: 4287, y: 2887 }, metres = unitScale(tile.level) * metresPerUnit((tile.y * EXTENT + 4000) * unitScale(tile.level));
    const road = { kind: 'road', tags: { highway: 'residential' }, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from([1000, 4000, 5000, 4000]), z: Float32Array.from([1000, 1000]) }] };
    const house = (gap: number) => {
        const [x, y] = [3000, 4000 + (2.5 + gap) / metres], [w, d] = [10 / metres, 8 / metres];
        return { kind: 'building', tags: { building: 'house' }, geometry: 'area' as const, parts: [{ outer: true, xy: Int32Array.from([x, y, x + w, y, x + w, y + d, x, y + d].map(Math.round)), z: null }] };
    };

    it('gives a road sidewalks on both sides where a building is near either, along the pieces within 20 m of it', () => {
        const [line] = features([road, house(10)], tile).lines, unit = metresPerUnit(line.xy[1]);
        expect(new Set(line.sidewalks)).toEqual(new Set([0, LEFT | RIGHT]));
        // The house's 10 m and about 16 m before and after it, in whole pieces of 8 m.
        const along = line.sidewalks!.reduce((sum, sides, v) => sum + (sides ? Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * unit : 0), 0);
        expect(Math.abs(along - 42)).toBeLessThan(8);
        expect(features([road], tile).lines[0].sidewalks).toEqual([0]);
    });

    it('lifts the ground through no road: a core takes the lower of two heights, and a stream\'s edge stays under it', () => {
        const body = functionOf(shader, 'stamp_at');
        expect(body).toContain('lo[WATER] = min(edge.x, hi[ROAD_CORE]);');
        expect(body).toContain('lo[ROAD_CORE] = min(lo[ROAD_CORE], hi[ROAD_CORE]);');
        expect(body).toContain('l = select(l, u, b == ROAD_CORE);');
    });

    it('keeps a sidewalk out of the bed of a line beside the road in every tile alike, and out of no culvert', () => {
        // Lines 5 m north of the road's middle, on its left: a bed there takes the left sidewalk's band.
        const y = Math.round(4000 - 5 / metres), beside = (kind: string, tags: Record<string, string>, xy: number[], heights = true) =>
            ({ kind, tags, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from(xy), z: heights ? Float32Array.from(xy.filter((_, i) => i % 2 === 0).map(() => 1001)) : null }] });
        const sides = (line: ReturnType<typeof beside>) => new Set(features([road, line, house(10)], tile).lines[0].sidewalks);
        expect(sides(beside('railway', { railway: 'rail' }, [1000, y, 5000, y]))).toEqual(new Set([0, RIGHT]));
        // A service road that leaves the road's first vertex, far from the house, as the tile that holds that vertex has it, and as one past it has it.
        expect(sides(beside('road', { highway: 'service' }, [1000, 4000, 1100, y, 5000, y]))).toEqual(new Set([0, RIGHT]));
        expect(sides(beside('road', { highway: 'service' }, [1100, y, 5000, y]))).toEqual(new Set([0, RIGHT]));
        // A culvert under the left sidewalk has no heights, and holds no ground.
        expect(sides(beside('stream', { waterway: 'stream', tunnel: 'culvert' }, [1000, Math.round(4000 - 3 / metres), 5000, Math.round(4000 - 3 / metres)], false))).toEqual(new Set([0, LEFT | RIGHT]));
        // Nor does a segment of a line that has a height at one end only.
        const unknown = beside('road', { highway: 'service' }, [1000, y, 5000, y]);
        expect(sides({ ...unknown, parts: [{ ...unknown.parts[0], z: Float32Array.from([1001, NaN]) }] })).toEqual(new Set([0, LEFT | RIGHT]));
    });

    it('lets a line that meets the street keep the sidewalk within 12 m of their vertex, as the corner shapes the mouth there, and no further', () => {
        const y = Math.round(4000 - 5 / metres), x = Math.round(3000 - 8 / metres), street = { ...road, parts: [{ outer: false, xy: Int32Array.from([1000, 4000, x, 4000, 5000, 4000]), z: Float32Array.from([1000, 1000, 1000]) }] };
        const lane = { kind: 'road', tags: { highway: 'service' }, geometry: 'line' as const, parts: [{ outer: false, xy: Int32Array.from([x, 4000, Math.round(x + 2 / metres), y, 5000, y]), z: Float32Array.from([1000, 1000, 1000]) }] };
        const line = features([street, lane, house(10)], tile).lines.find(({ xy }) => xy[1] === xy[xy.length - 1])!, world = x * unitScale(tile.level) + tile.x * EXTENT * unitScale(tile.level), unit = metresPerUnit(line.xy[1]);
        const ends = line.sidewalks!.flatMap((sides, v) => (sides === (LEFT | RIGHT) && (line.xy[2 * v] - world) * unit >= 0 ? [(line.xy[2 * v + 2] - world) * unit] : []));
        // In whole pieces of 8 m, whose middles decide: 3.25 m off the street, 12 m from the vertex is 11.5 m along.
        expect(ends[0]).toBeGreaterThan(11.5 - 4);
        expect(ends[0]).toBeLessThan(11.5 + 4);
        expect(ends).toHaveLength(1);
    });

    it('counts the ways out of a shared vertex: two where OSM splits a road, three at a junction', () => {
        const x = Math.round(3000 - 8 / metres), line = (xy: number[]) => ({ ...road, parts: [{ outer: false, xy: Int32Array.from(xy), z: Float32Array.from(xy.filter((_, i) => i % 2 === 0).map(() => 1000)) }] });
        const split = junctions(features([line([1000, 4000, x, 4000]), line([x, 4000, 5000, 4000])], tile).lines), t = junctions(features([line([1000, 4000, x, 4000, 5000, 4000]), line([x, 4000, x, 5000])], tile).lines);
        expect([...split.values()].map(({ ways }) => ways)).toEqual([2]);
        expect([...t.values()].map(({ ways }) => ways)).toEqual([3]);
    });

    it('fronts a house with its wall that faces the road, but for a house that a road reaches or none comes near', () => {
        const frontOf = (gap: number) => features([road, house(gap)], tile).buildings[0].front;
        expect(frontOf(15)).toEqual([0, -1]);
        expect(frontOf(1)).toBeUndefined();
        expect(frontOf(25)).toBeUndefined();
    });

    it('rounds the corners of a junction with a 5 m kerb, and runs a sidewalk round one or narrows it into a road without', () => {
        // The road runs on through a vertex where a side road leaves south, 8 m west of the house.
        const x = Math.round(3000 - 8 / metres), line = (xy: number[], highway = 'residential') => ({ ...road, tags: { highway }, parts: [{ outer: false, xy: Int32Array.from(xy), z: Float32Array.from(xy.filter((_, i) => i % 2 === 0).map(() => 1000)) }] });
        const walks = (highway: string) => {
            const lines = features([line([1000, 4000, x, 4000, 5000, 4000]), line([x, 4000, x, Math.round(4000 + 30 / metres)], highway), house(10)], tile).lines;
            const corners = cornersOf(lines), unit = metresPerUnit(corners[0].o[1]);
            expect(corners.length).toBe(2);
            for (const { o, a, b } of corners) for (const end of [a, b]) expect(Math.hypot(end[0] - o[0], end[1] - o[1]) * unit).toBeCloseTo(5, 1);
            return corners.map(({ walk }) => [...walk].sort()).sort();
        };
        expect(walks('residential')).toEqual([[1.5, 1.5], [1.5, 1.5]]);
        // A service road has no sidewalks.
        expect(walks('service')).toEqual([[0, 1.5], [0, 1.5]]);
    });
});
