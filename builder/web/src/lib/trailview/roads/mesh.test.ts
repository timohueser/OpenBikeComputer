import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { shaderFunctions } from '../gpu/shader-functions';
import { metresPerUnit, PATH, PROFILES, RAIL, ROAD, SHOULDER_M, type Line } from '../terrain/bins';
import { decodeFeatures, DEEPEST } from '../stream/features';
import { LEFT } from '../terrain/town';
import { meshBytes, OWNER, roadMesh } from './mesh';
import { lookWord, PieceBuilder, ROAD_VERTEX, type RoadPiece } from './piece';
import { CF, coreOf, MAT, sideOf, LOOK } from './section';
import { reachOf, stationsOf } from './stations';
import { cap, sweep, type Run } from './sweep';
import { onWay, waysOf } from './ways';

const shader = readFileSync(new URL('./corridor.wgsl', import.meta.url), 'utf8');
const root = new URL('../../../../tests/browser/trailview/fixture/features/', import.meta.url);

/** The corridor mesh of each owner of the fixture. */
const fixture = () => readdirSync(new URL(`${DEEPEST}/`, root)).flatMap(x => readdirSync(new URL(`${DEEPEST}/${x}/`, root)).flatMap(file => {
    const tile = { level: DEEPEST, x: Number(x), y: parseInt(file, 10) }, list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${DEEPEST}/${x}/${file}`, root)))));
    return Array.from({ length: 16 }, (_, i) => roadMesh(list, tile, { level: OWNER, x: tile.x * 4 + i % 4, y: tile.y * 4 + (i >> 2) }));
}));

const metres = metresPerUnit(2 ** 29);
/** A line east along world row 2^29, `length` metres long. */
const straight = (length: number, extra: Partial<Line> = {}): Line => {
    const xy = [2 ** 28, 2 ** 29, 2 ** 28 + Math.round(length / metres), 2 ** 29], z = [1000, 1000], zero = [0, 0];
    return { kind: ROAD, xy, z, h: 3, code: 2, ford: false, profile: 0, anchors: [true, true], plan: { tangent: zero, grade: zero, bank: zero, widen: zero }, ...extra };
};
const runOf = (line: Line, others: Line[] = []): Run => {
    const stations = stationsOf(line, () => line.h);
    return { id: 0, line, stations, reach: reachOf(stations), lift: 0, prio: 0, base: 0, ways: waysOf(others) };
};
/** The vertices of a piece as a test reads them. */
const decoded = ({ vertices, x, y }: RoadPiece) => {
    const words = new Uint32Array(vertices), floats = new Float32Array(vertices), ints = new Int16Array(vertices);
    return Array.from({ length: vertices.byteLength / ROAD_VERTEX }, (_, i) => ({
        x: x + ints[8 * i] / 3, y: y + ints[8 * i + 1] / 3, z: floats[4 * i + 1], mat: words[4 * i + 2] & 127, flags: (words[4 * i + 2] >> 11) & 31, half: (words[4 * i + 2] >>> 19) * 0.025,
        across: ((words[4 * i + 3] << 20) >> 20) * 16 / 2047, along: (words[4 * i + 3] >>> 12) / 64,
    }));
};

describe('the corridor meshes of the fixture', () => {
    const meshes = fixture(), pieces = meshes.flatMap(mesh => mesh.pieces);

    it('stay within the gates per owner, measured on the fixture of format 3: 186744 vertices and 5.04 MiB at most, 18519 and 0.51 MiB on average', () => {
        const count = meshes.map(mesh => mesh.pieces.reduce((sum, { vertices }) => sum + vertices.byteLength / ROAD_VERTEX, 0)), bytes = meshes.map(meshBytes);
        expect(Math.max(...count)).toBeLessThanOrEqual(190_000);
        expect(Math.max(...bytes)).toBeLessThanOrEqual(5.2 * 2 ** 20);
        expect(count.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(19_000);
        expect(bytes.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(0.53 * 2 ** 20);
    });

    it('hold 16 bytes per vertex, indices in range, and a far range of about a fifth of the near one', () => {
        for (const { vertices, indices, near, far, casters } of pieces) {
            expect(vertices.byteLength % ROAD_VERTEX).toBe(0);
            expect(vertices.byteLength / ROAD_VERTEX).toBeLessThanOrEqual(65536);
            expect(indices.every(index => index < vertices.byteLength / ROAD_VERTEX)).toBe(true);
            expect(indices.byteLength % 4).toBe(0);
            expect(near.first + near.count).toBe(far.first);
            expect(far.first + far.count).toBeLessThanOrEqual(indices.length);
            expect(far.vertices).toBeLessThanOrEqual(near.vertices);
            // Faces on the ground cast nothing, and every face so far lies on the ground.
            expect(casters.count).toBe(0);
        }
        const sum = (field: 'near' | 'far') => pieces.reduce((total, piece) => total + piece[field].vertices, 0);
        expect(sum('far')).toBeLessThan(0.25 * sum('near'));
    });
});

describe('the sweep', () => {
    /** The faces of a way between its first stations, and the vertices that they stand on. */
    const swept = (line: Line, others: Line[] = []) => {
        const b = new PieceBuilder(line.xy[0], line.xy[1]), run = runOf(line, others);
        sweep(b, b.near, run, 0, 1, false);
        return { indices: [...b.near], vertices: decoded(b.piece(0)) };
    };

    it('winds each face counter-clockwise from above, or to the line where it stands upright, and takes its material from its first vertex', () => {
        const { indices, vertices } = swept(straight(40, { sidewalks: [LEFT], profile: 8 }));
        for (let t = 0; t < indices.length; t += 3) {
            const [p, q, r] = [indices[t], indices[t + 1], indices[t + 2]].map(i => vertices[i]);
            // World y runs south.
            const area = (q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x);
            if (Math.abs(area) > 1e-6) expect(area, `face ${t / 3}`).toBeGreaterThan(0);
            const mats = [p, q, r].map(v => v.mat), out = Math.max(...[p, q, r].map(v => Math.abs(v.across)));
            // The face's own material: the first vertex lies at its outer edge.
            expect(Math.abs(p.across)).toBeCloseTo(p.mat >= LOOK.ASPHALT ? Math.abs(p.across) : out, 3);
            expect(mats).toContain(p.mat);
        }
    });

    it('shares a vertex between the faces that meet in a point of the cross-section', () => {
        const { indices, vertices } = swept(straight(40));
        expect(new Set(indices).size).toBe(vertices.length);
        // Two sides of three band rows and a shoulder, and the five points of the carriageway, at two stations.
        expect(vertices.length).toBe(2 * (5 + 2 * (1 + 4)));
    });

    it('holds the road\'s plane and the core\'s edge in a band\'s vertex, and its metres from the line', () => {
        const line = straight(40), { vertices } = swept(line), edge = coreOf(line, 1, 0, line.h), rows = vertices.filter(v => v.flags & CF.SAMPLE);
        expect(rows.length).toBeGreaterThan(0);
        for (const { z, half, across } of rows) {
            expect(z).toBeCloseTo(1000, 3);
            expect(half).toBeCloseTo(edge, 1);
            expect(Math.abs(across)).toBeGreaterThan(edge);
        }
        expect(vertices.filter(v => v.flags & CF.MESH).every(v => v.mat === MAT.GROUND)).toBe(true);
    });

    it('sinks a band\'s row over another way\'s carriageway, by the geometry of the tile', () => {
        const line = straight(40), other = { ...straight(40), xy: [line.xy[0], line.xy[1] - Math.round(5 / metres), line.xy[2], line.xy[3] - Math.round(5 / metres)] };
        const { vertices } = swept(line, [other]);
        expect(onWay(waysOf([other]), line.xy[0] + 100, line.xy[1] - Math.round(5 / metres))).toBe(true);
        expect(vertices.some(v => v.flags & CF.UNDER)).toBe(true);
        expect(swept(line).vertices.some(v => v.flags & CF.UNDER)).toBe(false);
    });

    it('rounds a dead end at either end with rings that hold their radial metres, wound as the sweep', () => {
        const line = straight(40), run = runOf(line), edge = coreOf(line, 1, 0, line.h);
        for (const [j, dir] of [[0, -1], [run.stations.length - 1, 1]] as const) {
            const s = run.stations[j], b = new PieceBuilder(s.x, s.y);
            cap(b, b.near, run, j, dir, false);
            const vertices = decoded(b.piece(0));
            for (const v of vertices.filter(({ flags }) => flags & CF.SAMPLE)) {
                expect(v.across).toBeGreaterThan(edge);
                expect(Math.hypot(v.x - s.x, v.y - s.y) * metres).toBeCloseTo(v.across, 0);
            }
            // The cap lies past the end, along the way out.
            for (const v of vertices) expect((v.x - s.x) * dir).toBeGreaterThanOrEqual(-1);
            for (let t = 0; t < b.near.length; t += 3) {
                const [p, q, r] = [b.near[t], b.near[t + 1], b.near[t + 2]].map(i => vertices[i]);
                expect((q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x), `face ${t / 3}`).toBeGreaterThan(-1e-3);
            }
        }
    });
});

describe('the cross-section', () => {
    it('ends its core where the stamp does: at the shoulder of the one table, or past a sidewalk', () => {
        for (const [surface, code] of [['main', 1], ['road', 2], ['gravel', 3], ['dirt', 4], ['grass', 5]] as const) {
            expect(coreOf(straight(1, { code }), 1, 0, 3)).toBeCloseTo(3 + SHOULDER_M[surface]!, 6);
        }
        expect(coreOf(straight(1, { sidewalks: [LEFT], profile: Object.keys(PROFILES).indexOf('residential') }), 1, 0, 3)).toBeCloseTo(4.5, 6);
        expect(coreOf(straight(1, { kind: PATH, code: 6 }), 1, 0, 1)).toBe(1);
    });

    it('ends in a row under the mesh, on paths nearer, and a track spills its ballast', () => {
        const rows = (line: Line) => sideOf(line, 1, 0, line.h).filter(({ flags }) => flags);
        expect(rows(straight(1)).map(({ flags }) => flags)).toEqual([CF.SAMPLE, CF.SAMPLE, CF.SAMPLE, CF.MESH]);
        expect(rows(straight(1, { kind: PATH, code: 6, h: 1 })).at(-1)!.a).toBeCloseTo(2.6);
        expect(sideOf(straight(1, { kind: RAIL, code: 9, h: 1.9 }), 1, 0, 1.9)[0].mat).toBe(LOOK.BALLAST);
    });
});

describe('corridor.wgsl', () => {
    it('reads the vertex as piece.ts packs it', () => {
        const number = (name: string) => Number(new RegExp(`const ${name}: [fu]32 = ([\\d.]+)u?;`).exec(shader)![1]);
        expect(number('CORRIDOR_STEP')).toBeCloseTo(1 / 3, 6);
        expect(number('ACROSS_STEP_M')).toBeCloseTo(16 / 2047, 6);
        expect(number('ALONG_STEP')).toBe(1 / 64);
        expect(lookWord(5, 9, 3, 2, number('HALF_STEP_M') * 80)).toBe((5 | (9 << number('CODE_SHIFT')) | (3 << number('FLAGS_SHIFT')) | (2 << number('PRIO_SHIFT')) | (80 << number('HALF_SHIFT'))) >>> 0);
        expect(number('MAT_GROUND')).toBe(MAT.GROUND);
        expect(number('MAT_SIDEWALK')).toBe(MAT.SIDEWALK);
        expect(number('LOOK_FIRST')).toBe(LOOK.ASPHALT);
        for (const [name, value] of Object.entries(CF)) expect(number(`CF_${name}`)).toBe(value);
    });

    it('dives a band only on a face steeper than the cones, measured from the core\'s edge', () => {
        const { band_dive: dive } = shaderFunctions<{ band_dive: (ground: number, plane: number, out: number) => number }>(shader, ['band_dive']);
        expect(dive(1000.5, 1000, 1)).toBe(0);
        expect(dive(1000.9, 1000, 1)).toBe(1.5);
        // A sidewalk does not dilute the slope: `out` counts from the core's edge.
        expect(dive(1000.9, 1000, 3)).toBe(0);
        expect(dive(1000.5, 1000, 0.1)).toBe(1.5);
    });
});
