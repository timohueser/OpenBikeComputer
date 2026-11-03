import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import baseline from '../../../../test-support/trailview/census-baseline.json' with { type: 'json' };
import { countLine, fixtureTiles, gaps, objectsOnWays, shortSidewalks, tornFaces, vertices } from '../../../../test-support/trailview/census';
import { functionOf, shaderFunctions } from '../gpu/shader-functions';
import { cramped, features, FORD, marked, MARKED, metresPerUnit, mitre, PATH, PROFILES, RAIL, ROAD, SHOULDER_M, shoulderOf, SURFACES, walkOf, type Line, type Node } from '../terrain/bins';
import { DEEPEST } from '../stream/features';
import { FAN_A, FAN_B, fanWord, pivotsOf, PIVOT_STEP_M } from '../terrain/pivots';
import { LEFT, RIGHT } from '../terrain/town';
import { CHILD, size } from '../terrain/lod';
import { stepSide } from './cores';
import { REACH_M, type Meeting } from './arms';
import { jointsOf } from './joints';
import { cornerFaces, mouthFaces } from './junction-faces';
import { meshBytes, planOf } from './mesh';
import { DETAIL_END_M, RANGE_M } from './roads';
import { FINE_SHIFT, lookWord, PART_FINE, PART_GRADE, PART_TURN, PieceBuilder, ROAD_VERTEX, type PieceRange, type RoadPiece } from './piece';
import { NARROW_GAUGE_M, NARROW_SLEEPER_HALF_M, RAIL_GAUGE_M, SLEEPER_HALF_M } from './parts';
import { carriageway, cellOf, CF, drawnCode, LIFT_M, lowered, LOOK, lookOf, MAT, sideOf, SPILL_M } from './section';
import { across, reachOf, rungsOf, stationIndex, stationsOf, type Station } from './stations';
import { daylightOf } from './skirt';
import { cap, sweep, type Run } from './sweep';
import { onWay, waysOf } from './ways';

const shader = readFileSync(new URL('./corridor.wgsl', import.meta.url), 'utf8');
const looks = ['looks', 'track-looks', 'details', 'cover'].map(name => readFileSync(new URL(`./${name}.wgsl`, import.meta.url), 'utf8')).join('');
const tiles = fixtureTiles();
/** The ways of a tile: each line's stations and those that its far range keeps. */
const waysIn = ({ tile, list }: typeof tiles[number]) => planOf(features(list, tile)).runs;
const squareOf = ({ x, y }: { x: number; y: number }, level: number) => `${Math.floor(x / size(level))},${Math.floor(y / size(level))}`;
/** The plan of a range of a piece: per grid cell of 2^10 world units, its triangles by their first vertex. */
function rangeCells(piece: RoadPiece, { first, count }: PieceRange) {
    const v = vertices(piece), at = (i: number) => [v.x(i), v.y(i)], cells = new Map<string, number[][][]>();
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
    rungsOf(line, stations, pivotsOf(line, () => line.h));
    return { id: 0, line, stations, reach: reachOf(stations), lift: 0, base: 0, ways: waysOf(others) };
};
/** The metres of a cell of the finest terrain mesh at the test's lines. */
const CELL_M = cellOf(straight(1));
/** The vertices of a piece as a test reads them. */
const decoded = (piece: RoadPiece) => {
    const v = vertices(piece);
    return Array.from({ length: v.count }, (_, i) => ({
        x: v.x(i), y: v.y(i), z: v.z(i), mat: v.word(i) & 127, flags: (v.word(i) >> 11) & 31, half: ((v.word(i) >>> 19) & 511) * 0.025,
        across: ((v.place(i) << 20) >> 20) * 16 / 2047, along: (v.place(i) >>> 12) / 64,
    }));
};

describe('the corridor meshes of the fixture', () => {
    const meshes = tiles.flatMap(tile => tile.meshes), pieces = meshes.flatMap(mesh => mesh.pieces);

    it('stay within the gates per owner, measured on the fixture of format 4 with its junctions, track details, plain ends, the returns of lesser ways, sidewalk ends and the fans and rows of tight bends: 243692 vertices and instances and 6.46 MiB at most, 21304 and 0.575 MiB on average', () => {
        const count = meshes.map(mesh => mesh.pieces.reduce((sum, { vertices }) => sum + vertices.byteLength / ROAD_VERTEX, 0)), bytes = meshes.map(meshBytes);
        expect(Math.max(...count)).toBeLessThanOrEqual(243_800);
        expect(Math.max(...bytes)).toBeLessThanOrEqual(6.47 * 2 ** 20);
        expect(count.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(21_350);
        expect(bytes.reduce((a, b) => a + b) / meshes.length).toBeLessThanOrEqual(0.576 * 2 ** 20);
    });

    it('hold 16 bytes per vertex, indices in range, and a far range of about a fifth of the near one', () => {
        for (const { level, vertices, indices, near, far, casters, over, parts } of pieces) {
            expect(vertices.byteLength % ROAD_VERTEX).toBe(0);
            const count = vertices.byteLength / ROAD_VERTEX - parts.reduce((sum, part) => sum + part.count, 0);
            // The frustum test counts all that a piece reaches past its square.
            expect(new Int16Array(vertices, 0, 8 * count).reduce((most, step, i) => (i % 8 < 2 ? Math.max(most, Math.abs(step) / 3) : most), 0)).toBeLessThanOrEqual(size(level) / 2 + over + 1);
            expect(count).toBeLessThanOrEqual(65536);
            expect(indices.every(index => index < count)).toBe(true);
            expect(indices.byteLength % 4).toBe(0);
            expect(near.first + near.count).toBe(far.first);
            expect(far.first + far.count).toBeLessThanOrEqual(indices.length);
            expect(far.vertices).toBeLessThanOrEqual(near.vertices);
            // Faces on the ground cast nothing; a detail's do.
            expect(casters.count).toBe(level === CHILD ? 0 : near.count);
        }
        const sum = (field: 'near' | 'far') => pieces.filter(({ level }) => level === CHILD).reduce((total, piece) => total + piece[field].vertices, 0);
        expect(sum('far')).toBeLessThan(0.25 * sum('near'));
    });

    it('give the census the core of each step as the sweep draws it: a sidewalk that ends at a vertex ends there', () => {
        // A residential road east, 40 m with a sidewalk on its left, then 40 m without.
        const step = Math.round(40 / metres), zero = [0, 0, 0];
        const line = straight(80, { xy: [2 ** 28, 2 ** 29, 2 ** 28 + step, 2 ** 29, 2 ** 28 + 2 * step, 2 ** 29], z: [1000, 1000, 1000], sidewalks: [LEFT, 0], profile: Object.keys(PROFILES).indexOf('residential'), anchors: [true, true, true], plan: { tangent: zero, grade: zero, bank: zero, widen: zero } });
        const st = stationsOf(line, () => line.h), [walked, bare] = [0, 1].map(v => sideOf(line, 1, v, line.h).edge);
        expect(walked).toBeGreaterThan(bare);
        for (let k = 0; k + 1 < st.length; k++) {
            const middle = (st[k].along + st[k + 1].along) / 2;
            expect(stepSide(line, st[k + 1], st[k], 1).edge, `the step at ${middle.toFixed(1)} m`).toBe(middle < 40 ? walked : bare);
        }
    });

    it('count the faults of the census on the fixture of format 4: 16942 gap samples (develop 17645, where a face of no area covers none), no object on a way, 116 short sidewalks (develop 107: the 9 more lie beside tight bends, where the viewer still places its own sidewalks)', () => {
        const counts = { gaps: gaps(tiles), 'objects-on-ways': objectsOnWays(tiles), 'short-sidewalks': shortSidewalks(tiles) };
        for (const [count, points] of Object.entries(counts)) console.log(countLine(count, points));
        expect(counts.gaps.length / 2).toBeLessThanOrEqual(16942);
        expect(counts['objects-on-ways']).toEqual([]);
        expect(counts['short-sidewalks'].length / 2).toBeLessThanOrEqual(116);
    }, 60_000);

    it('hold the torn faces of the census at census-baseline.json', () => {
        const torn = tornFaces(pieces), counts = Object.fromEntries(Object.entries(torn).map(([kind, points]) => [kind, points.length / 2]));
        for (const [kind, points] of Object.entries(torn)) console.log(countLine(`torn-${kind}`, points));
        for (const [kind, most] of Object.entries(baseline)) expect(counts[kind], `a change added torn ${kind}: fix the faces, do not raise the baseline`).toBeLessThanOrEqual(most);
        // A kind's count can hide a new torn face where a change also removes one: look at the worst places of road-census.ts.
        expect(counts, 'a change removed torn faces: lower census-baseline.json in the same commit').toEqual(baseline);
    }, 60_000);
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
        // Two sides of three skirt rows and a shoulder, and the five points of the carriageway, at two stations.
        expect(vertices.length).toBe(2 * (5 + 2 * (1 + 4)));
    });

    it('holds the road\'s plane and the core\'s edge in a skirt\'s vertex, and its metres from the line', () => {
        const line = straight(40), { vertices } = swept(line), edge = coreOf(line, line.h), rows = vertices.filter(v => v.flags & CF.SAMPLE);
        expect(rows.length).toBeGreaterThan(0);
        for (const { z, half, across } of rows) {
            expect(z).toBeCloseTo(1000, 3);
            expect(half).toBeCloseTo(edge, 1);
            expect(Math.abs(across)).toBeGreaterThan(edge);
        }
        expect(vertices.filter(v => v.flags & CF.MESH).every(v => v.mat === MAT.GROUND)).toBe(true);
    });

    it('sinks a skirt\'s row over another way\'s carriageway, by the geometry of the tile', () => {
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

describe('a tight bend', () => {
    /**
     * A residential road with a sidewalk on its right where the bend leaves it room: 12 m east, an arc of `radius` metres
     * that turns right by 150°, and 12 m on, so that its legs part as a hairpin's do; rising at `grade`.
     */
    const hairpin = (radius: number, grade = 0): Line => {
        const xy: number[] = [], tangent: number[] = [], along: number[] = [], turn = (5 * Math.PI) / 6, at = (x: number, y: number, heading: number) => {
            along.push(xy.length ? along[along.length - 1] + Math.hypot(x - (xy[xy.length - 2] - 2 ** 28) * metres, y - (xy[xy.length - 1] - 2 ** 29) * metres) : 0);
            xy.push(2 ** 28 + Math.round(x / metres), 2 ** 29 + Math.round(y / metres));
            tangent.push(heading);
        };
        for (let x = -12; x < 0; x += 2) at(x, 0, 0);
        const steps = Math.ceil(turn * radius / 0.5);
        for (let k = 0; k <= steps; k++) at(radius * Math.sin(turn * k / steps), radius * (1 - Math.cos(turn * k / steps)), turn * k / steps);
        for (let d = 2; d <= 12; d += 2) at(radius * Math.sin(turn) + d * Math.cos(turn), radius * (1 - Math.cos(turn)) + d * Math.sin(turn), turn);
        const zero = tangent.map(() => 0), z = along.map(m => 1000 + grade * m);
        const line = straight(1, { xy, z, anchors: tangent.map(() => true), plan: { tangent, grade: zero.map(() => grade), bank: zero, widen: zero }, profile: Object.keys(PROFILES).indexOf('residential') });
        return { ...line, sidewalks: cramped(line, line.h + walkOf(line)).map(bits => RIGHT & ~bits) };
    };
    const bends = [2, 4, 8].flatMap(radius => [0, 0.12].map(grade => ({ radius, grade, name: `radius ${radius}, grade ${grade}` })));
    /** The faces of a line's whole sweep, the vertices that they stand on, and their vertices' indices. */
    const swept = (line: Line) => {
        const run = runOf(line), b = new PieceBuilder(line.xy[0], line.xy[1]);
        for (let j = 0; j + 1 < run.stations.length; j++) sweep(b, b.near, run, j, j + 1, false);
        const vertices = decoded(b.piece(0)), ids = Array.from({ length: b.near.length / 3 }, (_, t) => [0, 1, 2].map(i => b.near[3 * t + i]));
        return { run, vertices, faces: ids.map(face => face.map(i => vertices[i])), ids };
    };
    /** The face of plan that lies over world point x, y, if any. */
    const under = (faces: ReturnType<typeof swept>['faces'], x: number, y: number) => faces.find(([p, q, r]) => {
        const d = [[p, q], [q, r], [r, p]].map(([a, b]) => (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x));
        return d.every(v => v >= 0) || d.every(v => v <= 0);
    });
    /** A face's unit normal, in metres, facing up. */
    const normal = ([p, q, r]: ReturnType<typeof swept>['faces'][number]) => {
        const [ux, uy, uz, vx, vy, vz] = [(q.x - p.x) * metres, (q.y - p.y) * metres, q.z - p.z, (r.x - p.x) * metres, (r.y - p.y) * metres, r.z - p.z];
        const n = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx], l = Math.hypot(...n);
        return n.map(c => (c / l) * Math.sign(n[2]));
    };

    it('has no sidewalk on its inside where the bend leaves no room, and one where it does', () => {
        // The legs have room; the middle of the half circle has room at 8 m only.
        for (const [radius, walked] of [[2, false], [4, false], [8, true]] as const) {
            const { sidewalks, z } = hairpin(radius);
            expect(sidewalks![0], `radius ${radius}`).toBe(RIGHT);
            expect(sidewalks![(z.length - 1) >> 1], `radius ${radius}`).toBe(walked ? RIGHT : 0);
        }
    });

    it('turns no face over, also where the bend is tighter than the carriageway', () => {
        for (const { radius, grade, name } of bends) {
            const { faces } = swept(hairpin(radius, grade));
            // World y runs south: a face that turns over winds the other way. Vertices round to a share of a world unit, so slivers of a few square centimetres may.
            const over = faces.reduce((sum, [p, q, r]) => sum + Math.max(0, -((q.x - p.x) * -(r.y - p.y) + (q.y - p.y) * (r.x - p.x)) / 2) * metres ** 2, 0);
            expect(over, name).toBeLessThan(0.01);
        }
    });

    it('stands no face on a point: none is a fin under 5 cm across with a height', () => {
        for (const { radius, grade, name } of bends) {
            const { faces } = swept(hairpin(radius, grade));
            const fins = faces.filter(face => {
                const across = Math.max(...face.map(p => Math.max(...face.map(q => Math.hypot(p.x - q.x, p.y - q.y))))) * metres, high = Math.max(...face.map(({ z }) => z)) - Math.min(...face.map(({ z }) => z));
                return across < 0.05 && high > 0.02;
            });
            expect(fins, name).toEqual([]);
        }
    });

    it('rounds its core\'s inner edge about a pivot where the bend is tighter than the edge and a radius, and never narrows the carriageway', () => {
        for (const { radius, grade, name } of bends) {
            const line = hairpin(radius, grade), pivots = pivotsOf(line, () => line.h), { run } = swept(line), core = line.h + shoulderOf(line);
            expect(pivots.length, name).toBe(radius < 8 ? 1 : 0);
            for (const s of run.stations) {
                expect(Math.abs(across(s, -line.h)[2]), name).toBeGreaterThanOrEqual(line.h - 1e-6);
                const [x, y] = across(s, -core);
                if (s.rung) expect(Math.hypot(x - pivots[0].x, y - pivots[0].y) * metres, name).toBeGreaterThan(pivots[0].r - 0.01);
            }
        }
    });

    it('turns the slope by at most 7° between two faces of the carriageway that share an edge, the crown included; across a fan\'s end, by the step of the inner edge\'s grade', () => {
        for (const { radius, grade, name } of bends) {
            const line = hairpin(radius, grade), { run, faces, ids } = swept(line), edges = new Map<string, number[]>(), [pivot] = pivotsOf(line, () => line.h);
            // At a fan's first and last cross-section the inner edge turns from the plain offset onto the arc, and its grade
            // steps from the way's to the way's times the ratio of the pivot's distances from the line and from the edge.
            const ends = run.stations.filter((s, j) => s.rung && !(run.stations[j - 1]?.rung && run.stations[j + 1]?.rung));
            const onEnd = (p: (typeof faces)[number][number], q: (typeof faces)[number][number]) => ends.some(s => [p, q].every(({ x, y }) => Math.abs((x - s.x) * s.rung!.uy - (y - s.y) * s.rung!.ux) * metres ** 2 < 0.02));
            const shoulder = shoulderOf(line), step = pivot ? ((Math.atan((grade * (line.h + shoulder + pivot.r)) / (shoulder + pivot.r)) - Math.atan(grade)) * 180) / Math.PI : 0;
            let [most, mostEnd] = [0, 0];
            faces.forEach((face, f) => {
                if (face.some(({ mat, across: a }) => mat !== lookOf(line) || Math.abs(a) > line.h + 0.01)) return;
                for (let k = 0; k < 3; k++) {
                    const [p, q] = [ids[f][k], ids[f][(k + 1) % 3]], key = `${Math.min(p, q)},${Math.max(p, q)}`;
                    for (const other of edges.get(key) ?? []) {
                        const [n, m] = [normal(face), normal(faces[other])], turn = (Math.acos(Math.min(1, n[0] * m[0] + n[1] * m[1] + n[2] * m[2])) * 180) / Math.PI;
                        if (onEnd(face[k], face[(k + 1) % 3])) mostEnd = Math.max(mostEnd, turn);
                        else most = Math.max(most, turn);
                    }
                    (edges.get(key) ?? edges.set(key, []).get(key)!).push(f);
                }
            });
            // At radius 2 m the legs of a road 3 m wide on each side merge for 10 m, and on a grade the fill between them ramps
            // steeper than a road: its faces stay under the twist of the torn-face check. At radius 4 m on a grade the crown's
            // middle creases by 6.7°, where the inner half runs along the rungs and the outer half along the normal.
            expect(most, name).toBeLessThan(radius === 2 && grade > 0 ? 30 : 7);
            expect(mostEnd, name).toBeLessThan(step + 3);
        }
    });

    it('lays a face over each texel that the stamp holds in its core, the filled area included, within a texel and at its height', () => {
        for (const { radius, grade, name } of bends) {
            const line = hairpin(radius, grade), { faces } = swept(line), texel = 0.4, [x0, y0] = line.xy, n = line.z.length, pivots = pivotsOf(line, () => line.h);
            // World y runs south: the right lies at a positive cross product.
            const core = (cross: number, v: number, fanned: boolean) => sideOf(line, cross > 0 ? -1 : 1, v, line.h, undefined, fanned).edge;
            const mitreOf = ([rx, ry]: number[], [dx, dy]: number[]): [number, number] => {
                const l = Math.hypot(rx, ry), m: [number, number] = [-ry / l, rx / l];
                return m[0] * dx + m[1] * dy >= 0 ? m : [-m[0], -m[1]];
            };
            // As `along` and `across_of` of stamp-core.wgsl for a road, copied by hand: inside a segment's mitre quad, a fan's
            // included, and within its core. The height of the core there.
            const held = (x: number, y: number) => {
                for (let v = 0; v + 1 < n; v++) {
                    const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), l = Math.hypot(bx - ax, by - ay), dir = [(bx - ax) / l, (by - ay) / l], ap = [x - ax, y - ay];
                    let [ma, mb] = [mitre(line, v), mitre(line, v + 1)];
                    const cross = dir[0] * ap[1] - dir[1] * ap[0], fan = fanWord(line, pivots, v), q = [(fan << 21) >> 21, (fan << 10) >> 21].map(c => (c * PIVOT_STEP_M) / metres);
                    const inner = fan !== 0 && cross * (dir[0] * q[1] - dir[1] * q[0]) > 0, [atA, atB] = [(fan & FAN_A) !== 0, fan >= FAN_B];
                    if (inner && atA) ma = mitreOf(q, dir);
                    if (inner && atB) mb = mitreOf([q[0] - (bx - ax), q[1] - (by - ay)], dir);
                    const da = (ap[0] * ma[0] + ap[1] * ma[1]) / Math.max(dir[0] * ma[0] + dir[1] * ma[1], 0.1), db = -((x - bx) * mb[0] + (y - by) * mb[1]) / Math.max(dir[0] * mb[0] + dir[1] * mb[1], 0.1);
                    if (da < 0 || db < 0) continue;
                    let lateral = Math.abs(cross) * metres;
                    if (inner && atA && atB) {
                        const qp = [ap[0] - q[0], ap[1] - q[1]], toward = dir[0] * q[1] - dir[1] * q[0], off = dir[0] * qp[1] - dir[1] * qp[0];
                        const share = Math.min(1e3, Math.max(1, -toward / (toward * off >= 0 ? -toward * 1e-3 : off))), dq = Math.hypot(qp[0], qp[1]) * metres, dc = share * dq, r = ((fan >>> 22) & 255) / 100;
                        const core = line.h + shoulderOf(line);
                        lateral = dq >= r ? (core * (dc - dq)) / Math.max(dc - r, 1e-3) : core + r - dq;
                    }
                    if (lateral <= core(cross, v, inner)) return line.z[v] + ((line.z[v + 1] - line.z[v]) * da) / Math.max(da + db, 1e-6);
                }
                return undefined;
            };
            const open: string[] = [], apart: string[] = [];
            let count = 0, filled = 0;
            for (let x = -12; x <= 8; x += texel / 2) for (let y = -6; y <= 12; y += texel / 2) {
                const [px, py] = [x0 + (x + 12) / metres, y0 + y / metres], z = held(px, py);
                if (z === undefined) continue;
                count++;
                const face = under(faces, px, py);
                if (face && face.every(({ mat, across: a }) => mat === lookOf(line) && Math.abs(a) <= line.h + 0.01)) {
                    // The carriageway lies LIFT_M over the core, and its crown at most CROWN of its half-width more.
                    const [p, q, r] = face, d = (q.x - p.x) * (r.y - p.y) - (r.x - p.x) * (q.y - p.y), wq = ((px - p.x) * (r.y - p.y) - (r.x - p.x) * (py - p.y)) / d, wr = ((q.x - p.x) * (py - p.y) - (px - p.x) * (q.y - p.y)) / d;
                    const over = p.z + wq * (q.z - p.z) + wr * (r.z - p.z) - z - LIFT_M;
                    if (over < -0.02 || over > 0.025 * line.h + 0.02) apart.push(`${x.toFixed(1)},${y.toFixed(1)}: ${over.toFixed(3)}`);
                    // The filled area lies farther than the half-width from the line.
                    if (Array.from({ length: n - 1 }, (_, v) => line.xy.slice(2 * v, 2 * v + 4)).every(([ax, ay, bx, by]) => {
                        const t = Math.max(0, Math.min(1, ((px - ax) * (bx - ax) + (py - ay) * (by - ay)) / ((bx - ax) ** 2 + (by - ay) ** 2 || 1)));
                        return Math.hypot(px - ax - t * (bx - ax), py - ay - t * (by - ay)) * metres > line.h + 0.1;
                    })) filled++;
                }
                const near = !!face || [[1, 0], [-1, 0], [0, 1], [0, -1]].some(([i, j]) => under(faces, px + (i * texel) / metres, py + (j * texel) / metres));
                if (!near) open.push(`${x.toFixed(1)},${y.toFixed(1)}`);
            }
            expect(count).toBeGreaterThan(1000);
            expect(open, name).toEqual([]);
            expect(apart, name).toEqual([]);
            if (radius < line.h + 1) expect(filled, name).toBeGreaterThan(0);
        }
    });
});

describe('a sidewalk\'s end', () => {
    it('ramps its kerb and its slab down to the road', () => {
        const line = straight(40, { sidewalks: [LEFT, 0], profile: Object.keys(PROFILES).indexOf('residential') });
        line.xy.splice(2, 0, line.xy[0] + Math.round(20 / metres), line.xy[1]);
        line.z.splice(1, 0, 1000);
        line.anchors = [true, true, true];
        line.plan = { tangent: [0, 0, 0], grade: [0, 0, 0], bank: [0, 0, 0], widen: [0, 0, 0] };
        const [{ stations }] = planOf({ lines: [line], lakes: [], seas: [], areas: [], buildings: [], barriers: [] }).runs, at = (m: number) => stations.find(s => Math.abs(s.along - m) < 0.05);
        expect(at(20)?.end?.[0]).toBe(1);
        expect(at(10)?.end).toBeUndefined();
        const end = lowered(sideOf(line, 1, 0, line.h).parts, 0, 1);
        for (const { dz, flags } of end) if (!flags) expect(dz).toBeCloseTo(LIFT_M, 6);
    });
});

describe('a piece', () => {
    it('holds a vertex past its range at the edge, and has no room left before 16-bit indices end', () => {
        const b = new PieceBuilder(0, 0);
        b.vertex(40000, -40000, 0, 0, 0, 0);
        expect(decoded(b.piece(0))[0]).toMatchObject({ x: (2 ** 17 - 1) / 12, y: -(2 ** 17) / 12 });
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
        expect(b.attempt(piece => { piece.vertex(9000, 0, 500, 0, 0, 0); make(6000)(piece); })).toBe(false);
        expect(b.count).toBe(60000);
        expect(b.piece(0)).toMatchObject({ low: 0, high: 0, over: 0 });
        expect(b.near.length).toBe(60000);
        expect(b.far.length).toBe(1);
        expect(new PieceBuilder(0, 0).attempt(make(6000))).toBe(true);
    });
});

describe('a kerb', () => {
    it('keeps its heading to 0.1° across a road at any angle, so a flat-shaded face never steps from light to shade', () => {
        const angle = 0.65, zero = [0, 0], reach = 200 / metres, [dx, dy] = [Math.round(reach * Math.cos(angle)), Math.round(reach * Math.sin(angle))];
        const line = straight(1, { xy: [2 ** 28, 2 ** 29, 2 ** 28 + dx, 2 ** 29 + dy], h: 2.5, sidewalks: [LEFT], profile: Object.keys(PROFILES).indexOf('residential'), plan: { tangent: [Math.atan2(dy, dx), Math.atan2(dy, dx)], grade: zero, bank: zero, widen: zero } });
        const run = runOf(line), b = new PieceBuilder(line.xy[0], line.xy[1]);
        for (let j = 0; j + 1 < run.stations.length; j++) sweep(b, b.near, run, j, j + 1, false);
        const edge = decoded(b.piece(0)).filter(({ mat, across }) => mat === MAT.KERB && Math.abs(across - 2.5) < 0.01).sort((p, q) => p.along - q.along);
        const headings = edge.slice(1).map((v, i) => Math.atan2(v.y - edge[i].y, v.x - edge[i].x) * 180 / Math.PI);
        expect(edge.length).toBeGreaterThan(90);
        expect(Math.max(...headings) - Math.min(...headings)).toBeLessThan(0.2);
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
        expect(rows(straight(1, { kind: PATH, code: 6, h: 1 })).at(-1)!.a).toBeCloseTo(1.8 + CELL_M);
        expect(sideOf(straight(1, { kind: RAIL, code: 9, h: 1.9 }), 1, 0, 1.9).parts[0].mat).toBe(LOOK.BALLAST);
    });

    it('has the same count of skirt rows at every daylight, out to a row a mesh cell past the daylight line under the mesh', () => {
        const round = (a: number) => Math.round(a * 100) / 100;
        const past = (line: Line, daylight: number) => {
            const { parts, edge } = sideOf(line, 1, 0, line.h, daylight), rows = parts.filter(({ flags }) => flags);
            expect(rows.map(({ flags }) => flags)).toEqual([CF.SAMPLE, CF.SAMPLE, CF.SAMPLE, CF.SAMPLE, CF.SAMPLE, CF.MESH]);
            return rows.map(({ a }) => round(a - edge));
        };
        // On flat ground the shares fall on the near rows: the rows of a skirt without a daylight.
        expect(past(straight(1), 0)).toEqual([0.35, 1, 1, 1, 2, round(2 + CELL_M)]);
        expect(past(straight(1), 6)).toEqual([0.35, 1, 2.4, 4.2, 6, round(6 + CELL_M)]);
        const path = straight(1, { kind: PATH, code: 6, h: 1 });
        expect(past(path, 0)).toEqual([0.3, 0.8, 0.8, 0.8, 0.8, round(0.8 + CELL_M)]);
        expect(past(path, 2.5)).toEqual([0.3, 0.8, 1, 1.75, 2.5, round(2.5 + CELL_M)]);
    });
});

describe('the skirt', () => {
    /** A road east whose ground climbs `slope` per metre from its middle line on both sides, as a cutting. */
    const cutting = (slope: number, extra: Partial<Line> = {}) => {
        const line = straight(40, extra), y0 = line.xy[1], ground = (_: number, y: number) => 1000 + slope * Math.abs(y - y0) * metres;
        const run = { ...runOf(line), daylight: daylightOf(line, stationsOf(line, () => line.h), ground) };
        return { line, run, edge: coreOf(line, line.h) };
    };

    it('reaches the daylight line, where the cone of the cut meets the ground', () => {
        const { tol, reach, cut } = Object.values(PROFILES)[0], { line, run, edge } = cutting(1.5);
        const over = (x: number) => 1.5 * (edge + x) - (tol + cut * x * reach / (reach - x));
        for (const most of [0, 1]) {
            const daylight = run.daylight(0, most);
            expect(over(daylight - 0.15)).toBeGreaterThan(0);
            expect(over(daylight + 0.15)).toBeLessThan(0);
        }
        // A plain road on flat ground, and on unknown ground, has a short skirt.
        expect(cutting(0).run.daylight(0, 0)).toBe(0);
        expect(daylightOf(line, run.stations, () => NaN)(0, 0)).toBe(0);
    });

    it('ends its daylight a step before a carriageway, whose corridor draws that ground', () => {
        // The other carriageway starts 4 m past this one's core, inside the cut's daylight; the search steps a metre, and stops a step before it.
        const { line, run, edge } = cutting(1.5), apart = edge + line.h + 4, other = { ...line, xy: line.xy.map((v, i) => (i % 2 ? v - Math.round(apart / metres) : v)) };
        const ended = daylightOf(line, run.stations, (_, y) => 1000 + 1.5 * Math.abs(y - line.xy[1]) * metres, waysOf([line, other]));
        expect(run.daylight(0, 0)).toBeGreaterThan(4);
        expect(ended(0, 0)).toBeLessThanOrEqual(4);
        expect(ended(0, 0)).toBeGreaterThan(2);
        expect(ended(0, 1)).toBe(run.daylight(0, 1));
    });

    it('draws its rows out to a mesh cell past the daylight line, and no more vertices than the flat skirt on flat ground', () => {
        const { line, run, edge } = cutting(1.5), b = new PieceBuilder(line.xy[0], line.xy[1]);
        sweep(b, b.near, run, 0, 1, false);
        const vertices = decoded(b.piece(0)), daylight = run.daylight(0, 0);
        const most = (flags: number) => Math.max(...vertices.filter(v => v.flags === flags && v.across > 0).map(v => v.across - edge));
        expect(most(CF.SAMPLE)).toBeCloseTo(daylight, 1);
        expect(most(CF.MESH)).toBeCloseTo(daylight + CELL_M, 1);
        const flat = cutting(0), plain = new PieceBuilder(line.xy[0], line.xy[1]), bare = new PieceBuilder(line.xy[0], line.xy[1]);
        sweep(plain, plain.near, flat.run, 0, 1, false);
        sweep(bare, bare.near, runOf(flat.line), 0, 1, false);
        expect([plain.near.length, decoded(plain.piece(0)).length]).toEqual([bare.near.length, decoded(bare.piece(0)).length]);
    });
});

describe('corridor.wgsl', () => {
    it('reads the vertex as piece.ts packs it', () => {
        const number = (name: string) => Number(new RegExp(`const ${name}: [fu]32 = ([\\d.]+)u?;`).exec(shader + looks)![1]);
        expect(number('CORRIDOR_STEP')).toBeCloseTo(1 / 12, 6);
        expect(number('ACROSS_STEP_M')).toBeCloseTo(16 / 2047, 6);
        expect(number('ALONG_STEP')).toBe(1 / 64);
        expect(number('FINE_SHIFT')).toBe(FINE_SHIFT);
        for (const [name, value] of Object.entries({ PART_FINE, PART_TURN, PART_GRADE, RAIL_GAUGE_M, NARROW_GAUGE_M, SLEEPER_HALF_M, NARROW_SLEEPER_HALF_M, DETAIL_END_M })) expect(number(name), name).toBe(value);
        expect(lookWord(5, 9, 3, number('HALF_STEP_M') * 80)).toBe((5 | (9 << number('CODE_SHIFT')) | (3 << number('FLAGS_SHIFT')) | (80 << number('HALF_SHIFT'))) >>> 0);
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

    it('holds the widths of the faces that give way to the ground at the mesh edge', () => {
        const number = (name: string) => Number(new RegExp(`const ${name}: f32 = ([\\d.]+);`).exec(shader + looks)![1]);
        expect(number('SHOULDER_MAIN_M')).toBe(SHOULDER_M.main);
        const oneWay = straight(1, { code: 2, oneway: 1 });
        expect(drawnCode(oneWay)).toBe(Object.keys(SURFACES).indexOf('oneway') + 1);
        expect(number('SHOULDER_ROAD_M')).toBe(shoulderOf(oneWay));
        expect(number('SHOULDER_ROAD_M')).toBe(SHOULDER_M.road);
        expect(number('SPILL_M')).toBe(SPILL_M);
        expect(number('EDGE_SLACK_M')).toBe(number('HALF_STEP_M'));
    });

    it('frays a narrow paved way only where no kerb or shoulder lies beside it', () => {
        const flags = (extra: Partial<Line>, h: number) => carriageway(straight(1, extra), h, 0)[0].flags;
        const off = CF.NO_LEFT_LINE | CF.NO_RIGHT_LINE;
        expect(flags({ code: 2 }, 1.5)).toBe(off);
        expect(flags({ code: 2 }, 2.5)).toBe(0);
        expect(flags({ code: Object.keys(SURFACES).indexOf('paving') + 1 }, 1.25)).toBe(0);
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
        const { paint_share: share } = shaderFunctions<{ paint_share: (distance: number) => number }>(features, ['paint_share']), scope = globalThis as { masks?: { held: number } };
        scope.masks = { held: 600 };
        expect([100, 560, 600, 5000].map(share)).toEqual([0, 0, 1, 1]);
        scope.masks = { held: 0 };
        expect(share(100)).toBe(1);
        delete scope.masks;
        expect(functionOf(shader, 'corridor_fragment')).toMatch(/if \(paint_draws\(.*\)\) \{\s+discard;/);
        expect(functionOf(paint, 'road_at')).toContain('!paint_draws(distance, pixel)');
    });
});
