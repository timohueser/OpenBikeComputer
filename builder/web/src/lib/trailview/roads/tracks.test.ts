import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { shaderFunctions } from '../gpu/shader-functions';
import { LIFT_M, metresPerUnit, PATH, PROFILES, RAIL, RAIL_CODE, ROAD, SURFACES, type Line } from '../terrain/bins';
import { DETAIL_END_M } from './roads';
import { LOG_HALF_M, PART, partMeshes, SLEEPER_TOP_M } from './parts';
import { PieceBuilder, ROAD_VERTEX, type RoadPiece } from './piece';
import { CF, LOOK } from './section';
import { CUT, reachOf, stationsOf } from './stations';
import type { Run } from './sweep';
import { tracks } from './tracks';

const y0 = 2 ** 29, metre = 1 / metresPerUnit(y0), code = (surface: keyof typeof SURFACES) => Object.keys(SURFACES).indexOf(surface) + 1;

/** A way `metres` long, east along world row 2^29, rising `rise`. */
function way(metres: number, rise: number, profile: string, extra: Partial<Line>): Line {
    return { kind: RAIL, xy: [2 ** 28, y0, 2 ** 28 + metres * metre, y0], z: [1000, 1000 + rise], h: 1.9, code: RAIL_CODE, ford: false, profile: Object.keys(PROFILES).indexOf(profile), anchors: [true, true], ...extra };
}

/** The piece of the details of a line, all of it in one square. */
function detailsOf(line: Line, cut: (along: number) => number | undefined = () => undefined): RoadPiece {
    const stations = stationsOf(line, () => line.h).map(s => ({ ...s, cut: cut(s.along) })), b = new PieceBuilder(stations[0].x, stations[0].y, 18, true), run: Run = { id: 0, line, stations, reach: reachOf(stations), lift: 0, base: 0 };
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

/** The instances of a piece: each one's part, its metres east of x0 and its height. */
function instances({ vertices, parts }: RoadPiece, x0: number) {
    const ints = new Int32Array(vertices), floats = new Float32Array(vertices), words = new Uint32Array(vertices);
    return parts.flatMap(({ part, first, count }) => Array.from({ length: count }, (_, k) => {
        const at = 4 * (first + k);
        return { part, east: (ints[at] + ((words[at + 3] >>> 16) & 15) / 16 - x0) / metre, z: floats[at + 2] };
    }));
}

describe('the steps of a path in a town', () => {
    const flight = (metres: number, rise: number) => read(detailsOf(way(metres, rise, 'steps', { kind: PATH, h: 1, code: code('paving') })));

    it('rise about 16 cm a step, with level treads', () => {
        const { list } = flight(6, 3.2), treads = list.filter(({ mat }) => mat === LOOK.TREAD), levels = [...new Set(treads.map(({ z }) => z.toFixed(3)))].map(Number).sort((a, b) => a - b);
        expect(levels).toHaveLength(20);
        for (let k = 1; k < levels.length; k++) expect(levels[k] - levels[k - 1]).toBeCloseTo(0.16, 2);
    });

    it('face each riser down the flight', () => {
        for (const rise of [3.2, -3.2]) {
            const { list } = flight(6, rise), piece = detailsOf(way(6, rise, 'steps', { kind: PATH, h: 1, code: code('paving') }));
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

describe('the steps of a trail', () => {
    const x0 = 2 ** 28;
    /** A flight through points [metres east, height], or `metres` long rising `rise`, and its logs. */
    const flight = (points: [number, number][], surface: keyof typeof SURFACES = 'gravel', h = 1, cut?: (along: number) => number | undefined) => {
        const line: Line = { ...way(1, 0, 'steps', { kind: PATH, h, code: code(surface) }), xy: points.flatMap(([east]) => [x0 + east * metre, y0]), z: points.map(([, z]) => z), anchors: points.map(() => true) };
        const piece = detailsOf(line, cut);
        return { piece, logs: instances(piece, x0).sort((a, b) => a.east - b.east) };
    };
    const straight = (metres: number, rise: number, surface?: keyof typeof SURFACES, h?: number) => flight([[0, 1000], [metres, 1000 + rise]], surface, h);
    const gaps = (logs: { east: number }[]) => logs.slice(1).map((log, k) => log.east - logs[k].east);
    /** A log's shift along the way, at most, either way. */
    const SHIFT = 0.06;

    it('lay a log at each 0.2 m of height in the world, and nothing else: the core is the tread', () => {
        const { piece, logs } = straight(15, 3);
        expect(piece.indices).toHaveLength(0);
        expect(logs).toHaveLength(16);
        logs.forEach(({ east, z }, k) => {
            expect(Math.abs(east - k)).toBeLessThan(SHIFT + 0.01);
            expect(Math.abs(z - 1000 - 0.2 * k)).toBeLessThan(0.02);
        });
    });

    it('lay logs no nearer than 0.9 m, so a gentle flight gets few, and a short flight gets one', () => {
        for (const [metres, rise, gap] of [[10, 1, 2], [6, 3.2, 1.125]]) {
            const { logs } = straight(metres, rise);
            expect(logs.length).toBeGreaterThan(2);
            for (const d of gaps(logs)) expect(Math.abs(d - gap)).toBeLessThan(2 * SHIFT + 0.01);
        }
        expect(straight(0.5, 0.3).logs).toHaveLength(1);
    });

    it('lay the same logs whichever way the flight is drawn, and in each tile that holds it', () => {
        const key = (logs: { east: number; z: number; part: number }[]) => logs.map(({ east, z, part }) => [east.toFixed(2), z.toFixed(2), part]);
        expect(key(flight([[0, 1000], [12, 1003]]).logs)).toEqual(key(flight([[12, 1003], [0, 1000]]).logs));
        const points = Array.from({ length: 9 }, (_, v): [number, number] => [5 * v, 1000 + 0.25 * 5 * v]), within = <T extends { east: number }>(logs: T[]) => logs.filter(({ east }) => east > 17 && east < 23);
        const [a, b] = [flight(points.slice(0, 6)).logs, flight(points.slice(3)).logs].map(within);
        expect(a.length).toBeGreaterThan(2);
        expect(key(a)).toEqual(key(b));
    });

    it('keep a log near a crest and no wide gap past it', () => {
        const { logs } = flight([[0, 1000], [10, 1003], [20, 1000]]);
        expect(Math.min(...logs.map(({ east }) => Math.abs(east - 10)))).toBeLessThan(1.5);
        expect(Math.max(...gaps(logs))).toBeLessThan(2.5);
        expect(Math.min(...gaps(logs))).toBeGreaterThan(0.9 - 2 * SHIFT);
    });

    it('stop at a road', () => {
        const { logs } = flight([[0, 1000], [40, 1006]], 'gravel', 1, along => (along >= 20 && along < 30 ? CUT.ROAD : undefined));
        expect(logs.filter(({ east }) => east > 20.5 && east < 29.9)).toEqual([]);
        expect(logs.some(({ east }) => east > 30)).toBe(true);
    });

    it('take logs within the tread, the longest that fits or the next shorter, on each unpaved surface', () => {
        for (const [surface, h, tread] of [['gravel', 1, 0.85], ['gravel', 0.5, 0.35], ['dirt', 1, 0.35], ['grass', 1, 0.35], ['path', 0.6, 0.35]] as const) {
            const halves = new Set(straight(10, 3, surface, h).logs.map(({ part }) => LOG_HALF_M[part - PART.LOG])), fits = LOG_HALF_M.filter(half => half <= tread);
            expect([...halves].sort(), surface).toEqual(fits.slice(-2));
        }
    });
});

describe('a road that stops a way', () => {
    const stop = (along: number) => (along >= 20 ? CUT.ROAD : undefined);

    it('stops its steps', () => {
        const line = way(40, 6, 'steps', { kind: PATH, h: 1, code: code('paving') }), piece = detailsOf(line, along => (along >= 20 && along < 30 ? CUT.ROAD : undefined));
        const { list } = read(piece), east = list.map(({ x }) => (x - line.xy[0]) * metresPerUnit(y0));
        expect(east.filter(e => e > 20.1 && e < 29.9)).toEqual([]);
        expect(east.some(e => e > 30)).toBe(true);
        expect(list.some(({ mat }) => mat === LOOK.TREAD)).toBe(true);
    });

    it('stops its rails and its sleepers', () => {
        const line = way(40, 0, 'rail', { sleepers: [0, 66] }), piece = detailsOf(line, stop), { list } = read(piece);
        expect(Math.max(...list.map(({ x }) => (x - line.xy[0]) * metresPerUnit(y0)))).toBeLessThan(23);
        expect(piece.parts[0].count).toBeLessThan(40);
    });
});

describe('the parts', () => {
    it('wind each face out, and stand the sleeper\'s top on the bed and a log\'s foot in the ground', () => {
        const { vertices, indices, ranges } = partMeshes(), floats = new Float32Array(vertices);
        expect(ranges).toHaveLength(PART.LOG + LOG_HALF_M.length);
        const point = (i: number) => [floats[4 * i], floats[4 * i + 1], floats[4 * i + 2]];
        const heights = (part: number) => Array.from(indices.subarray(ranges[part].first, ranges[part].first + ranges[part].count), i => point(i)[2]);
        expect(Math.max(...heights(PART.SLEEPER))).toBeCloseTo(SLEEPER_TOP_M, 5);
        // A way's core lies LIFT_M over its plane.
        expect(Math.min(...heights(PART.LOG))).toBeLessThan(LIFT_M);
        for (const { first, count } of ranges) {
            const corners = Array.from(indices.subarray(first, first + count), point), middle = [0, 1, 2].map(k => (Math.min(...corners.map(p => p[k])) + Math.max(...corners.map(p => p[k]))) / 2);
            for (let f = first; f < first + count; f += 3) {
                const [a, b, c] = [indices[f], indices[f + 1], indices[f + 2]].map(point), out = [0, 1, 2].map(k => (a[k] + b[k] + c[k]) / 3 - middle[k]);
                const u = [0, 1, 2].map(k => b[k] - a[k]), v = [0, 1, 2].map(k => c[k] - a[k]);
                // The frame across, ahead, up turns left-handed: a face seen from outside runs clockwise in its numbers.
                const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                if (Math.hypot(...n) > 1e-9) expect(n[0] * out[0] + n[1] * out[1] + n[2] * out[2], `face ${f / 3}`).toBeLessThan(0);
            }
        }
    });
});

describe('a detail\'s reach', () => {
    const looks = readFileSync(new URL('./track-looks.wgsl', import.meta.url), 'utf8');

    it('ends where the look of its way has taken over, at the range that culls it', () => {
        const { detail_fade: fade, detail_painted: painted } = shaderFunctions<{ detail_fade: (distance: number) => number; detail_painted: (distance: number) => boolean }>(looks, ['detail_fade', 'detail_painted']);
        expect(fade(140)).toBe(0);
        expect(fade(DETAIL_END_M)).toBe(1);
        // A pixel that a face discards shows the paint in full, so a rail keeps its strength through the fade.
        expect([139, 140, 165].map(painted)).toEqual([false, true, true]);
    });
});
