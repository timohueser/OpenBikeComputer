import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { functionOf, shaderFunctions } from '../gpu/shader-functions';
import { features, FORD, marked, MARKED, metresPerUnit, PATH, PROFILES, RAIL, ROAD, SHOULDER_M, SURFACES, type Line, type Node } from '../terrain/bins';
import { decodeFeatures, DEEPEST } from '../stream/features';
import { LEFT } from '../terrain/town';
import { CHILD, OWNER, size } from '../terrain/lod';
import { REACH_M, type Meeting } from './arms';
import { jointsOf } from './joints';
import { cornerFaces, mouthFaces } from './junction-faces';
import { meshBytes, planOf, roadMesh } from './mesh';
import { RANGE_M } from './roads';
import { lookWord, PieceBuilder, ROAD_VERTEX, type PieceRange, type RoadPiece } from './piece';
import { CF, LOOK, lookOf, MAT, sideOf } from './section';
import { reachOf, stationIndex, stationsOf, type Station } from './stations';
import { cap, sweep, type Run } from './sweep';
import { onWay, waysOf } from './ways';

const shader = readFileSync(new URL('./corridor.wgsl', import.meta.url), 'utf8');
const looks = readFileSync(new URL('./looks.wgsl', import.meta.url), 'utf8') + readFileSync(new URL('./details.wgsl', import.meta.url), 'utf8');
const root = new URL('../../../../tests/browser/trailview/fixture/features/', import.meta.url);

/** The tiles of the fixture, each with the corridor mesh of its owners. */
const tiles = readdirSync(new URL(`${DEEPEST}/`, root)).flatMap(x => readdirSync(new URL(`${DEEPEST}/${x}/`, root)).map(file => {
    const tile = { level: DEEPEST, x: Number(x), y: parseInt(file, 10) }, list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${DEEPEST}/${x}/${file}`, root)))));
    return { tile, list, meshes: Array.from({ length: 16 }, (_, i) => roadMesh(list, tile, { level: OWNER, x: tile.x * 4 + i % 4, y: tile.y * 4 + (i >> 2) })) };
}));
/** The ways of a tile: each line's stations and those that its far range keeps. */
const waysIn = ({ tile, list }: typeof tiles[number]) => planOf(features(list, tile)).runs;
const squareOf = ({ x, y }: { x: number; y: number }, level: number) => `${Math.floor(x / size(level))},${Math.floor(y / size(level))}`;
/** The plan of a range of a piece: per grid cell of 2^10 world units, its triangles by their first vertex. */
function rangeCells(piece: RoadPiece, { first, count }: PieceRange) {
    const steps = new Int16Array(piece.vertices), at = (i: number) => [piece.x + steps[8 * i] / 3, piece.y + steps[8 * i + 1] / 3], cells = new Map<string, number[][][]>();
    for (let t = first; t < first + count; t += 3) {
        const face = [at(piece.indices[t]), at(piece.indices[t + 1]), at(piece.indices[t + 2])], name = `${face[0][0] >> 10},${face[0][1] >> 10}`;
        (cells.get(name) ?? cells.set(name, []).get(name)!).push(face);
    }
    return cells;
}
/** Whether a plan holds a point in a face, its faces starting within a cell of the point. */
function covers(cells: ReturnType<typeof rangeCells>, x: number, y: number): boolean {
    const side = (a: number[], b: number[]) => (x - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (y - b[1]);
    for (let i = -1; i <= 1; i++) for (let j = -1; j <= 1; j++) {
        for (const [a, b, c] of cells.get(`${(x >> 10) + i},${(y >> 10) + j}`) ?? []) {
            const d = [side(a, b), side(b, c), side(c, a)];
            if (d.every(v => v <= 0) || d.every(v => v >= 0)) return true;
        }
    }
    return false;
}

const metres = metresPerUnit(2 ** 29);
const coreOf = (line: Line, h: number) => sideOf(line, 1, 0, h).edge;
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
    const meshes = tiles.flatMap(tile => tile.meshes), pieces = meshes.flatMap(mesh => mesh.pieces);

    it('stay within the gates per owner, measured on the fixture of format 3 with its junctions: 205977 vertices and 5.54 MiB at most, 20097 and 0.55 MiB on average', () => {
        const count = meshes.map(mesh => mesh.pieces.reduce((sum, { vertices }) => sum + vertices.byteLength / ROAD_VERTEX, 0)), bytes = meshes.map(meshBytes);
        expect(Math.max(...count)).toBeLessThanOrEqual(206_000);
        expect(Math.max(...bytes)).toBeLessThanOrEqual(5.6 * 2 ** 20);
        expect(count.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(20_500);
        expect(bytes.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(0.55 * 2 ** 20);
    });

    it('hold 16 bytes per vertex, indices in range, and a far range of about a fifth of the near one', () => {
        for (const { vertices, indices, near, far, casters, over } of pieces) {
            expect(vertices.byteLength % ROAD_VERTEX).toBe(0);
            // The frustum test counts all that a piece reaches past its square.
            expect(new Int16Array(vertices).reduce((most, step, i) => (i % 8 < 2 ? Math.max(most, Math.abs(step) / 3) : most), 0)).toBeLessThanOrEqual(size(CHILD) / 2 + over + 1);
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

describe('where a way leaves a child', () => {
    it('leaves no gap, whichever of the two children draws near and which far', () => {
        let crossings = 0;
        const open: string[] = [];
        for (const fixtureTile of tiles) {
            const pieces = fixtureTile.meshes.flatMap(mesh => mesh.pieces), plans = new Map<RoadPiece, { near: ReturnType<typeof rangeCells>; far: ReturnType<typeof rangeCells> }>();
            const planOf = (piece: RoadPiece) => plans.get(piece) ?? plans.set(piece, { near: rangeCells(piece, piece.near), far: rangeCells(piece, piece.far) }).get(piece)!;
            const of = (square: string) => pieces.filter(piece => squareOf(piece, CHILD) === square).map(planOf);
            const here = (s: Station) => squareOf(s, DEEPEST) === `${fixtureTile.tile.x},${fixtureTile.tile.y}`;
            for (const { stations } of waysIn(fixtureTile)) {
                for (let j = 0; j + 1 < stations.length; j++) {
                    const [a, b] = [stations[j], stations[j + 1]], [A, B] = [squareOf(a, CHILD), squareOf(b, CHILD)];
                    if (A === B || !here(a) || !here(b)) continue;
                    crossings++;
                    const [nearA, nearB] = [of(A), of(B)];
                    // Up to 12 m either side of the crossing, as far as the way stays in the two children.
                    for (let i = Math.max(0, j - 6); i <= j + 6 && i + 1 < stations.length; i++) {
                        const [p, q] = [stations[i], stations[i + 1]], x = (p.x + q.x) / 2, y = (p.y + q.y) / 2;
                        if (![A, B].includes(squareOf(p, CHILD)) || ![A, B].includes(squareOf(q, CHILD))) continue;
                        if (!nearA.some(({ near }) => covers(near, x, y)) && !nearB.some(({ far }) => covers(far, x, y))) open.push(`near ${A}, far ${B}`);
                        if (!nearA.some(({ far }) => covers(far, x, y)) && !nearB.some(({ near }) => covers(near, x, y))) open.push(`far ${A}, near ${B}`);
                    }
                }
            }
        }
        expect(open).toEqual([]);
        expect(crossings).toBeGreaterThan(500);
    }, 30_000);
});

describe('where a way leaves a tile', () => {
    it('takes the same stations in both tiles, in the near range and in the far range', () => {
        const ways = new Map(tiles.map(fixtureTile => [`${fixtureTile.tile.x},${fixtureTile.tile.y}`, waysIn(fixtureTile)]));
        const place = ({ x, y, z, h, bank, cut }: Station) => [Math.round(x), Math.round(y), z.toFixed(2), h.toFixed(2), bank.toFixed(3), cut ?? 0].join(), sets = new Map<string, { near: Set<string>; far: Set<string> }>();
        for (const [name, list] of ways) sets.set(name, { near: new Set(list.flatMap(({ stations }) => stations.map(place))), far: new Set(list.flatMap(({ stations, far }) => far.map(j => place(stations[j])))) });
        let near = 0, far = 0;
        for (const [name, list] of ways) {
            for (const { stations, far: kept } of list) {
                for (let j = 0; j + 1 < stations.length; j++) {
                    const other = squareOf(stations[j + 1], DEEPEST), set = sets.get(other);
                    if (squareOf(stations[j], DEEPEST) !== name || other === name || !set) continue;
                    near++;
                    expect(set.near.has(place(stations[j + 1])), `near into ${other}`).toBe(true);
                }
                for (let k = 0; k + 1 < kept.length; k++) {
                    const other = squareOf(stations[kept[k + 1]], DEEPEST), set = sets.get(other);
                    if (squareOf(stations[kept[k]], DEEPEST) !== name || other === name || !set) continue;
                    far++;
                    expect(set.far.has(place(stations[kept[k + 1]])), `far into ${other}`).toBe(true);
                }
            }
        }
        expect(Math.min(near, far)).toBeGreaterThan(50);
    });
});

describe('a junction that two tiles hold', () => {
    const metresOut = ({ x, y, unit }: Meeting, { level, x: tx, y: ty }: Node) => {
        const side = size(level), dx = Math.max(tx * side - x, 0, x - (tx + 1) * side), dy = Math.max(ty * side - y, 0, y - (ty + 1) * side);
        return Math.hypot(dx, dy) * unit;
    };
    const metres = (values: number[]) => values.map(v => Math.abs(v).toFixed(2));
    /** The faces of a junction about its point: their index lists and look words, and their heights, which a way's lift moves by the tile's order of the lines. */
    const facesOf = (x: number, y: number, draw: (b: PieceBuilder) => void) => {
        const b = new PieceBuilder(x, y);
        draw(b);
        const { vertices } = b.piece(0), words = new Uint32Array(vertices), floats = new Float32Array(vertices);
        return { shape: [...b.near, -1, ...b.far, -1, ...words.filter((_, i) => i % 4 > 1)].join(), z: Array.from({ length: words.length / 4 }, (_, i) => floats[4 * i + 1]) };
    };
    /** What a tile plans: per thing of a junction its name, the point that it belongs to, and its faces. */
    const planned = ({ tile, list }: typeof tiles[number]) => {
        const { junctions } = planOf(features(list, tile)), named = new Map<string, { point: Meeting; faces?: ReturnType<typeof facesOf> }>();
        for (const corner of junctions.corners) {
            const { meeting, p, q, r, ends, meet } = corner, { x, y } = meeting;
            named.set(['corner', x, y, p.angle.toFixed(2), q.angle.toFixed(2), r.toFixed(2), ...metres([ends[0] - p.along, ends[1] - q.along, meet[0] - p.along, meet[1] - q.along])].join(), { point: meeting, faces: facesOf(x, y, b => cornerFaces(b, corner)) });
        }
        for (const mouth of junctions.mouths) {
            const { way, far, reach, left: { meeting } } = mouth, { x, y } = meeting;
            named.set(['mouth', x, y, way.angle.toFixed(2), ...metres([far - way.along, reach])].join(), { point: meeting, faces: facesOf(x, y, b => mouthFaces(b, mouth)) });
        }
        for (const { way, at, walk } of junctions.ends) {
            const [x, y] = way.line.xy.slice(2 * way.v, 2 * way.v + 2);
            named.set(['end', x, y, way.angle.toFixed(2), ...metres([at - way.along]), walk].join(), { point: { x, y, unit: way.unit } as Meeting });
        }
        return { tile, named };
    };

    it('has the same corners, mouths, path ends and faces in both, within the reach of its plan', () => {
        const plans = tiles.map(planned);
        let corners = 0, mouths = 0, ends = 0;
        plans.forEach(({ tile, named }, i) => {
            for (const [name, { point, faces }] of named) {
                plans.forEach((other, k) => {
                    if (k === i || metresOut(point, tile) > REACH_M || metresOut(point, other.tile) > REACH_M) return;
                    const theirs = other.named.get(name);
                    expect(theirs, name).toBeDefined();
                    corners += +name.startsWith('corner');
                    mouths += +name.startsWith('mouth');
                    ends += +name.startsWith('end');
                    if (!faces) return;
                    expect(theirs!.faces!.shape, name).toBe(faces.shape);
                    faces.z.forEach((z, v) => expect(Math.abs(z - theirs!.faces!.z[v]), name).toBeLessThan(0.02));
                });
            }
        });
        expect(Math.min(corners, mouths, ends)).toBeGreaterThan(5);
    }, 30_000);
});

describe('the joints of the fixture', () => {
    it('hold a split road\'s edges on one height: at most 1 cm by the final stations, with the lifts; 5.5 mm measured on the fixture of format 3', () => {
        let joints = 0, worst = 0;
        for (const fixtureTile of tiles) {
            const { runs } = planOf(features(fixtureTile.list, fixtureTile.tile)), edges = (run: Run, s: Station, side: number) => s.z + run.lift + s.bank * side * s.h;
            for (const [run, ends] of jointsOf(features(fixtureTile.list, fixtureTile.tile).lines, runs)) {
                ends.forEach((joint, end) => {
                    const s = run.stations[end ? run.stations.length - 1 : 0];
                    if (!joint || squareOf(s, DEEPEST) !== `${fixtureTile.tile.x},${fixtureTile.tile.y}`) return;
                    const o = joint.run.stations[joint.end ? joint.run.stations.length - 1 : 0];
                    joints++;
                    for (const side of [1, -1]) worst = Math.max(worst, Math.abs(edges(run, s, side) - edges(joint.run, o, side * joint.flip)));
                });
            }
        }
        expect(joints).toBeGreaterThan(400);
        expect(worst).toBeLessThanOrEqual(0.01);
    });
});

describe('the mouths of the fixture', () => {
    it('leave a side arm\'s edges with no step: a grade of 0.89 at most, measured on the fixture of format 3, over the stations past a mouth', () => {
        let worst = 0;
        for (const { list, tile } of tiles) {
            for (const { way: { run: { stations }, step }, far } of planOf(features(list, tile)).junctions.mouths) {
                const j = stationIndex(stations, far), edge = (s: Station, side: number) => s.z + s.bank * side * s.h;
                for (let k = 0; k < 6 && stations[j + (k + 1) * step]; k++) {
                    const s = stations[j + k * step], t = stations[j + (k + 1) * step], d = Math.abs(t.along - s.along);
                    if (d > 0.05) for (const side of [1, -1]) worst = Math.max(worst, Math.abs(edge(s, side) - edge(t, side)) / d);
                }
            }
        }
        expect(worst).toBeLessThanOrEqual(1);
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
        const line = straight(40, { sidewalks: [LEFT], profile: Object.keys(PROFILES).indexOf('residential') }), { indices, vertices } = swept(line);
        for (let t = 0; t < indices.length; t += 3) {
            const [p, q, r] = [indices[t], indices[t + 1], indices[t + 2]].map(i => vertices[i]);
            // World y runs south.
            const area = (q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x);
            if (Math.abs(area) > 1e-6) expect(area, `face ${t / 3}`).toBeGreaterThan(0);
            // Past the carriageway the first vertex is the point of the section that ends the face, in that point's material.
            const part = sideOf(line, p.across > 0 ? 1 : -1, 0, line.h).parts.find(({ a, dz }) => Math.abs(a - Math.abs(p.across)) < 0.01 && Math.abs(1000 + dz - p.z) < 1e-3);
            expect(p.mat, `face ${t / 3}`).toBe(part?.mat ?? lookOf(line));
            if (part) expect(Math.abs(p.across)).toBeCloseTo(Math.max(...[p, q, r].map(v => Math.abs(v.across))), 3);
        }
    });

    it('shares a vertex between the faces that meet in a point of the cross-section', () => {
        const { indices, vertices } = swept(straight(40));
        expect(new Set(indices).size).toBe(vertices.length);
        // Two sides of three band rows and a shoulder, and the five points of the carriageway, at two stations.
        expect(vertices.length).toBe(2 * (5 + 2 * (1 + 4)));
    });

    it('holds the road\'s plane and the core\'s edge in a band\'s vertex, and its metres from the line', () => {
        const line = straight(40), { vertices } = swept(line), edge = coreOf(line, line.h), rows = vertices.filter(v => v.flags & CF.SAMPLE);
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
        const line = straight(40), run = runOf(line), edge = coreOf(line, line.h);
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

describe('a piece', () => {
    it('holds a vertex past its range at the edge, and has no room left before 16-bit indices end', () => {
        const b = new PieceBuilder(0, 0);
        b.vertex(40000, -40000, 0, 0, 0, 0);
        expect(decoded(b.piece(0))[0]).toMatchObject({ x: 32767 / 3, y: -32768 / 3 });
        while (b.roomy) b.vertex(0, 0, 0, 0, 0, 0);
        expect(b.count).toBeLessThan(65536);
        expect(b.count).toBeGreaterThan(64000);
    });

    it('leaves no trace of faces that do not fit, so a junction never wraps a 16-bit index', () => {
        const b = new PieceBuilder(0, 0);
        const make = (vertices: number) => (piece: PieceBuilder) => {
            for (let k = 0; k < vertices; k++) piece.near.push(piece.vertex(0, 0, 0, 0, 0, 0));
            piece.far.push(0);
        };
        expect(b.attempt(make(60000))).toBe(true);
        expect(b.attempt(make(6000))).toBe(false);
        expect(b.count).toBe(60000);
        expect(b.near.length).toBe(60000);
        expect(b.far.length).toBe(1);
        expect(new PieceBuilder(0, 0).attempt(make(6000))).toBe(true);
    });
});

describe('the cross-section', () => {
    it('ends its core where the stamp does: at the shoulder of the one table, or past a sidewalk', () => {
        for (const [surface, code] of [['main', 1], ['road', 2], ['gravel', 3], ['dirt', 4], ['grass', 5]] as const) {
            expect(coreOf(straight(1, { code }), 3)).toBeCloseTo(3 + SHOULDER_M[surface]!, 6);
        }
        expect(coreOf(straight(1, { sidewalks: [LEFT], profile: Object.keys(PROFILES).indexOf('residential') }), 3)).toBeCloseTo(4.5, 6);
        expect(coreOf(straight(1, { kind: PATH, code: 6 }), 1)).toBe(1);
    });

    it('ends in a row under the mesh, on paths nearer, and a track spills its ballast', () => {
        const rows = (line: Line) => sideOf(line, 1, 0, line.h).parts.filter(({ flags }) => flags);
        expect(rows(straight(1)).map(({ flags }) => flags)).toEqual([CF.SAMPLE, CF.SAMPLE, CF.SAMPLE, CF.MESH]);
        expect(rows(straight(1, { kind: PATH, code: 6, h: 1 })).at(-1)!.a).toBeCloseTo(2.6);
        expect(sideOf(straight(1, { kind: RAIL, code: 9, h: 1.9 }), 1, 0, 1.9).parts[0].mat).toBe(LOOK.BALLAST);
    });
});

describe('corridor.wgsl', () => {
    it('reads the vertex as piece.ts packs it', () => {
        const number = (name: string) => Number(new RegExp(`const ${name}: [fu]32 = ([\\d.]+)u?;`).exec(shader + looks)![1]);
        expect(number('CORRIDOR_STEP')).toBeCloseTo(1 / 3, 6);
        expect(number('ACROSS_STEP_M')).toBeCloseTo(16 / 2047, 6);
        expect(number('ALONG_STEP')).toBe(1 / 64);
        expect(lookWord(5, 9, 3, 2, number('HALF_STEP_M') * 80)).toBe((5 | (9 << number('CODE_SHIFT')) | (3 << number('FLAGS_SHIFT')) | (2 << number('PRIO_SHIFT')) | (80 << number('HALF_SHIFT'))) >>> 0);
        for (const [name, value] of Object.entries(MAT)) expect(number(`MAT_${name}`)).toBe(value);
        for (const [name, value] of Object.entries(CF)) expect(number(`CF_${name}`)).toBe(value);
        for (const [, name, value] of looks.matchAll(/^const LOOK_(\w+): u32 = (\d+)u;/gm)) expect(LOOK[name as keyof typeof LOOK]).toBe(Number(value));
    });

    it('holds the values of the other sides of the shaders', () => {
        const number = (source: string, name: string) => Number(new RegExp(`const ${name}: [fu]32 = ([\\d.]+)u?;`).exec(source)![1]);
        const code = (surface: keyof typeof SURFACES) => Object.keys(SURFACES).indexOf(surface) + 1;
        const paint = readFileSync(new URL('../ground/roads.wgsl', import.meta.url), 'utf8'), stamp = readFileSync(new URL('../terrain/stamp-core.wgsl', import.meta.url), 'utf8');
        for (const [name, surface] of [['GRAVEL', 'gravel'], ['DIRT', 'dirt'], ['GRASS', 'grass'], ['PATH', 'path'], ['ONEWAY', 'oneway']] as const) expect(number(paint, `CODE_${name}`)).toBe(code(surface));
        expect([number(stamp, 'DIRT_CODE'), number(stamp, 'PATH_CODE')]).toEqual([code('dirt'), code('path')]);
        expect(number(stamp, 'MARKED')).toBe(MARKED);
        expect(number(stamp, 'FORD')).toBe(FORD);
        expect(number(looks, 'CORRIDOR_RANGE_M')).toBe(RANGE_M);
    });

    it('marks the roads that wear lines: main classes on asphalt', () => {
        const road = (name: string, extra: Partial<Line> = {}) => marked(straight(1, { profile: Object.keys(PROFILES).indexOf(name), code: 2, ...extra }));
        for (const name of ['motorway', 'trunk_link', 'primary', 'secondary', 'tertiary', 'tertiary_link']) expect(road(name), name).toBe(true);
        for (const name of ['unclassified', 'residential', 'service', 'track']) expect(road(name), name).toBe(false);
        expect(road('tertiary', { code: 3 })).toBe(false);
        expect(road('tertiary', { surface: 'sett' })).toBe(false);
        expect(road('tertiary', { surface: 'concrete:plates' })).toBe(false);
    });

    it('lays concrete strips only on a track', () => {
        const look = (name: string) => lookOf(straight(1, { profile: Object.keys(PROFILES).indexOf(name), code: 2, h: 1, surface: 'concrete' }));
        expect(look('track')).toBe(LOOK.STRIPS);
        expect(look('footway')).toBe(LOOK.CONCRETE);
        expect(look('service')).toBe(LOOK.CONCRETE);
    });

    it('hands the pixels of a way over to the paint in the last metres that corridors hold', () => {
        const features = readFileSync(new URL('../terrain/features.wgsl', import.meta.url), 'utf8'), paint = readFileSync(new URL('../ground/roads.wgsl', import.meta.url), 'utf8');
        const { paint_share: share } = shaderFunctions<{ paint_share: (distance: number) => number }>(features, ['paint_share']), scope = globalThis as { masks?: { corridors: number } };
        scope.masks = { corridors: 600 };
        expect([100, 560, 600, 5000].map(share)).toEqual([0, 0, 1, 1]);
        scope.masks = { corridors: 0 };
        expect(share(100)).toBe(1);
        delete scope.masks;
        expect(functionOf(shader, 'corridor_fragment')).toMatch(/if \(paint_draws\(.*\)\) \{\s+discard;/);
        expect(functionOf(paint, 'road_at')).toContain('!paint_draws(distance, pixel)');
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
