import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { shaderFunctions } from '../gpu/shader-functions';
import { metresPerUnit, PATH, PROFILES, RAIL, RAIL_CODE, ROAD, SURFACES, type Line } from '../terrain/bins';
import { DETAIL_END_M } from './roads';
import { PART, partMeshes, SLEEPER_TOP_M } from './parts';
import { PieceBuilder, ROAD_VERTEX, type RoadPiece } from './piece';
import { CF, LOOK } from './section';
import { reachOf, stationsOf } from './stations';
import type { Run } from './sweep';
import { tracks } from './tracks';

const y0 = 2 ** 29, metre = 1 / metresPerUnit(y0), code = (surface: keyof typeof SURFACES) => Object.keys(SURFACES).indexOf(surface) + 1;

/** A way `metres` long, east along world row 2^29, rising `rise`. */
function way(metres: number, rise: number, profile: string, extra: Partial<Line>): Line {
    return { kind: RAIL, xy: [2 ** 28, y0, 2 ** 28 + metres * metre, y0], z: [1000, 1000 + rise], h: 1.9, code: RAIL_CODE, ford: false, profile: Object.keys(PROFILES).indexOf(profile), anchors: [true, true], ...extra };
}

/** The piece of the details of a line, all of it in one square. */
function detailsOf(line: Line): RoadPiece {
    const stations = stationsOf(line, () => line.h), b = new PieceBuilder(stations[0].x, stations[0].y, 18, true), run: Run = { id: 0, line, stations, reach: reachOf(stations), lift: 0, prio: 0, base: 0, ways: { boxes: [], cells: new Map(), marks: new Uint32Array(0), query: 0, segments: [] } };
    tracks(run, () => b);
    return b.piece(0);
}

/** The vertices of a piece, and of each of its faces' first vertex the material. */
function read({ vertices, indices, near, x, y, parts }: RoadPiece) {
    const words = new Uint32Array(vertices), floats = new Float32Array(vertices), steps = new Int16Array(vertices), count = vertices.byteLength / ROAD_VERTEX - parts.reduce((sum, part) => sum + part.count, 0);
    const list = Array.from({ length: count }, (_, i) => ({
        x: x + (steps[8 * i] * 4 + ((words[4 * i + 2] >>> 28) & 3)) / 12, y: y + (steps[8 * i + 1] * 4 + (words[4 * i + 2] >>> 30)) / 12, z: floats[4 * i + 1], mat: words[4 * i + 2] & 127, flags: (words[4 * i + 2] >> 11) & 31,
    }));
    return { list, faces: Array.from(indices.subarray(near.first, near.first + near.count)).filter((_, i) => i % 3 === 0).map(i => list[i].mat) };
}

describe('the details of a track', () => {
    const sleepers = (count: number) => [0, count];

    it('lie sleepers at each whole count, as instances that keep their place and their grade', () => {
        const piece = detailsOf(way(60, 3, 'rail', { sleepers: sleepers(100) }));
        expect(piece.parts).toHaveLength(1);
        const [{ part, first, count }] = piece.parts, ints = new Int32Array(piece.vertices), floats = new Float32Array(piece.vertices), words = new Uint32Array(piece.vertices);
        expect(part).toBe(PART.SLEEPER);
        expect(count).toBe(100);
        for (let k = 0; k < count; k++) {
            const at = 4 * (first + k), x = ints[at] + ((words[at + 3] >>> 16) & 15) / 16;
            expect((x - 2 ** 28) / metre).toBeCloseTo(k * 0.6, 1);
            expect(ints[at + 1]).toBe(y0);
            expect(floats[at + 2]).toBeCloseTo(1000 + 0.05 * (k * 0.6), 2);
            // The left of a way east is north: a quarter turn from east.
            expect((words[at + 3] & 65535) / 65536).toBeCloseTo(0.25, 3);
            expect((words[at + 3] >> 24) / 64).toBeCloseTo(0.05, 1);
        }
    });

    it('sweep two rails at the gauge, with heads of their own look, and stay within 2 mm of it', () => {
        for (const [surface, gauge] of [['rail', 0.7525], ['narrow', 0.53]] as const) {
            const { list, faces } = read(detailsOf(way(10, 0, 'rail', { code: code(surface), sleepers: sleepers(16) })));
            const heads = list.filter(({ mat }) => mat === LOOK.RAIL_HEAD);
            expect(heads.length).toBeGreaterThan(0);
            // A head's face starts at its right edge; the left of a way east is north, where world y falls.
            for (const { y } of heads) expect(Math.min(...[gauge, -gauge].map(rail => Math.abs((y0 - y) / metre - (rail - 0.031))))).toBeLessThan(0.002);
            expect(new Set(faces)).toEqual(new Set([LOOK.RAIL, LOOK.RAIL_HEAD]));
        }
    });

    it('add a rack on a rack railway, on timber, with nothing a plain track has not', () => {
        for (const [surface, gauge] of [['rack', 'rack'], ['narrow-rack', 'rack']] as const) {
            const piece = detailsOf(way(10, 2, gauge, { code: code(surface), sleepers: sleepers(16) }));
            expect(new Set(read(piece).faces)).toEqual(new Set([LOOK.RAIL, LOOK.RAIL_HEAD, LOOK.RACK]));
            expect(piece.parts.map(({ part }) => part)).toEqual([surface === 'rack' ? PART.TIMBER : PART.TIMBER_NARROW]);
        }
    });

    it('add a funicular\'s cable on rollers every 8 m', () => {
        const piece = detailsOf(way(39, 20, 'funicular', { code: code('narrow'), sleepers: sleepers(66) }));
        expect(read(piece).faces).toContain(LOOK.CABLE);
        expect(piece.parts.find(({ part }) => part === PART.ROLLER)?.count).toBe(5);
    });

    it('cast, and a carriageway has no details', () => {
        const piece = detailsOf(way(10, 0, 'rail', { sleepers: sleepers(16) }));
        expect(piece.casters).toEqual(piece.near);
        expect(piece.near.count).toBeGreaterThan(0);
        for (const line of [way(10, 0, 'motorway', { kind: ROAD, code: code('main'), sleepers: undefined }), way(10, 0, 'path', { kind: PATH, code: code('path'), sleepers: undefined })]) expect(detailsOf(line).indices).toHaveLength(0);
    });
});

describe('the steps of a path', () => {
    const flight = (metres: number, rise: number, surface: keyof typeof SURFACES = 'path') => read(detailsOf(way(metres, rise, 'steps', { kind: PATH, h: 1, code: code(surface) })));

    it('rise about 16 cm a step, with level treads', () => {
        const { list } = flight(6, 3.2), treads = list.filter(({ mat }) => mat === LOOK.TREAD), levels = [...new Set(treads.map(({ z }) => z.toFixed(3)))].map(Number).sort((a, b) => a - b);
        expect(levels).toHaveLength(20);
        for (let k = 1; k < levels.length; k++) expect(levels[k] - levels[k - 1]).toBeCloseTo(0.16, 2);
    });

    it('face each riser down the flight', () => {
        for (const rise of [3.2, -3.2]) {
            const { list } = flight(6, rise), piece = detailsOf(way(6, rise, 'steps', { kind: PATH, h: 1, code: code('path') }));
            const risers = Array.from({ length: piece.indices.length / 6 }, (_, q) => [...piece.indices.subarray(6 * q, 6 * q + 6)].map(i => list[i])).filter(([a, b, c]) => a.mat === LOOK.RISER && Math.abs(a.x - b.x) + Math.abs(b.x - c.x) < 1e-6);
            expect(risers.length).toBeGreaterThan(10);
            for (const [a, b, c] of risers) {
                // World y runs south: the east part of the face's normal, from its winding.
                const east = (a.y - b.y) * (c.z - a.z) + (b.z - a.z) * (c.y - a.y);
                expect(Math.sign(east)).toBe(rise > 0 ? -1 : 1);
            }
        }
    });

    it('take cheeks down to the drawn ground, each face first at its tread', () => {
        const { list } = flight(6, 3.2), feet = list.filter(({ flags }) => flags !== 0);
        expect(feet.length).toBeGreaterThan(0);
        for (const { mat, flags } of feet) expect([mat, flags]).toEqual([LOOK.RISER, CF.MESH]);
    });
});

describe('the parts', () => {
    it('wind each face out, and stand the sleeper\'s top on the bed', () => {
        const { vertices, indices, ranges } = partMeshes(), floats = new Float32Array(vertices);
        expect(ranges.map(({ count }) => count)).toEqual(Object.values(PART).map(() => 30));
        expect(Math.max(...Array.from({ length: ranges[0].vertices }, (_, i) => floats[4 * i + 2]))).toBeCloseTo(SLEEPER_TOP_M, 5);
        const point = (i: number) => [floats[4 * i], floats[4 * i + 1], floats[4 * i + 2]];
        for (let f = 0; f < indices.length; f += 6) {
            const [a, b, c] = [indices[f], indices[f + 1], indices[f + 2]].map(point), mid = [0, 1, 2].map(k => (a[k] + b[k] + c[k]) / 3), out = [mid[0], mid[1], mid[2] - SLEEPER_TOP_M + 0.06];
            const u = [0, 1, 2].map(k => b[k] - a[k]), v = [0, 1, 2].map(k => c[k] - a[k]);
            // The frame across, ahead, up turns left-handed: a face seen from outside runs clockwise in its numbers.
            const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            expect(n[0] * out[0] + n[1] * out[1] + n[2] * out[2], `face ${f / 6}`).toBeLessThan(0);
        }
    });
});

describe('a detail\'s reach', () => {
    const looks = readFileSync(new URL('./track-looks.wgsl', import.meta.url), 'utf8');

    it('ends where the look of its way has taken over, at the range that culls it', () => {
        const { detail_fade: fade } = shaderFunctions<{ detail_fade: (distance: number) => number }>(looks, ['detail_fade']);
        expect(fade(140)).toBe(0);
        expect(fade(DETAIL_END_M)).toBe(1);
    });
});
