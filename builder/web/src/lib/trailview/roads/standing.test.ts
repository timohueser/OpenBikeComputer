import { describe, expect, it } from 'vitest';
import { metresPerUnit, PROFILES, ROAD, SURFACES, type Barrier, type Line } from '../terrain/bins';
import { size } from '../terrain/lod';
import { LEFT } from '../terrain/town';
import { roadside } from './details';
import type { GroundAt } from './ground';
import { PieceBuilder } from './piece';
import { CF, MAT } from './section';
import { barriersOf, coneOf, standingOf, zonesOf } from './standing';
import { reachOf as foldsOf, stationsOf, type Station } from './stations';
import { waysOf } from './ways';

const Y = 2 ** 29, unit = metresPerUnit(Y), LENGTH = 200;
/** World x of a point `m` metres along the line. */
const xOf = (m: number) => 2 ** 28 + m / unit;
/** Metres along of world x. */
const alongOf = (x: number) => (x - 2 ** 28) * unit;

/** A level secondary road 200 m long due east at 1000 m, 3 m wide each side: its left lies to the north. */
function road(overrides: Partial<Line> = {}): { line: Line; stations: Station[] } {
    const line: Line = {
        kind: ROAD, xy: [xOf(0), Y, xOf(LENGTH), Y], z: [1000, 1000], h: 3, code: Object.keys(SURFACES).indexOf('main') + 1, ford: false, profile: Object.keys(PROFILES).indexOf('secondary'),
        anchors: [true, true], ...overrides,
    };
    return { line, stations: stationsOf(line, () => line.h) };
}

/** Ground that lies `left` metres over the road on its left, and `right` on its right, from 60 to 100 m along, level elsewhere. */
const bump = (left: number, right: number): GroundAt => (x, y) => {
    const m = alongOf(x);
    return 1000 + (m >= 60 && m <= 100 ? (y < Y ? left : right) : 0);
};
const far = (stations: Station[]) => new Float64Array(stations.length).fill(Infinity);
const stands = (line: Line, stations: Station[], side: 1 | -1, ground: GroundAt, mouths: Float64Array = far(stations), towns = mouths, mapped: Barrier[] = []) => standingOf(line, stations, side, ground, mouths, towns, barriersOf(mapped));
/** The value of a station-wise field at metres along. */
const at = (stations: Station[], values: Float64Array, m: number) => values[stations.findIndex(s => s.along >= m)];

describe('what stands beside a road', () => {
    it('stands a rail where the ground drops, whole within 4 m of it, ramped down over 6 m', () => {
        const { line, stations } = road(), { rail, wall } = stands(line, stations, 1, bump(-3, 0));
        expect(at(stations, rail, 80)).toBe(1);
        expect(at(stations, rail, 56.5)).toBe(1);
        const ramp = at(stations, rail, 52);
        expect(ramp).toBeGreaterThan(0);
        expect(ramp).toBeLessThan(1);
        expect(at(stations, rail, 40)).toBe(0);
        expect(wall.every(share => share === 0)).toBe(true);
        expect(stands(line, stations, -1, bump(-3, 0)).rail.every(share => share === 0)).toBe(true);
    });

    it('stands a wall where the ground climbs, as high as the cut that the stamp leaves plus 0.3 m', () => {
        const { line, stations } = road(), { rail, wall, high } = stands(line, stations, -1, bump(0, 5));
        expect(at(stations, wall, 80)).toBe(1);
        expect(at(stations, wall, 54)).toBeGreaterThan(0);
        expect(at(stations, wall, 54)).toBeLessThan(1);
        expect(at(stations, wall, 40)).toBe(0);
        expect(at(stations, high, 80)).toBeCloseTo(coneOf(line, 2.2) + 0.3, 3);
        expect(rail.every(share => share === 0)).toBe(true);
    });

    it('reads no ground past a mouth, so a cut in the mouth raises no stub', () => {
        const { line, stations } = road(), mouths = zonesOf(stations, j => Math.abs(stations[j].along - 95) < 8);
        const { wall } = stands(line, stations, -1, (x, y) => 1000 + (alongOf(x) > 90 && alongOf(x) < 100 && y > Y ? 5 : 0), mouths);
        expect(at(stations, wall, 92)).toBe(0);
        expect(at(stations, wall, 84)).toBe(0);
        expect(at(stations, wall, 110)).toBe(0);
    });

    it('stops and starts nothing but through its ramp, up to a mouth', () => {
        const { line, stations } = road(), mouths = zonesOf(stations, j => stations[j].along > 90);
        const { wall } = stands(line, stations, -1, bump(0, 5), mouths);
        expect(at(stations, wall, 80)).toBe(1);
        expect(at(stations, wall, 88)).toBeLessThan(0.5);
        expect(at(stations, wall, 91)).toBe(0);
    });

    it('takes a mapped barrier first: a rail on level ground, and no rail test beside a mapped wall', () => {
        const { line, stations } = road(), north = Y - 4.5 / unit;
        const rail: Barrier = { xy: [xOf(80), north, xOf(120), north], wall: false };
        expect(at(stations, stands(line, stations, 1, bump(0, 0), undefined, undefined, [rail]).rail, 100)).toBe(1);
        const wall: Barrier = { xy: [xOf(70), north, xOf(90), north], wall: true, height: 2.5 };
        const { rail: dropped } = stands(line, stations, 1, bump(-3, 0), undefined, undefined, [wall]);
        expect(at(stations, dropped, 80)).toBe(0);
        const climbs = stands(line, stations, 1, bump(1, 0), undefined, undefined, [wall]);
        expect(at(stations, climbs.wall, 80)).toBe(1);
        expect(at(stations, climbs.high, 80)).toBe(2.5);
    });

    it('stands nothing beside a sidewalk, and ramps down before it', () => {
        const { line, stations } = road({ xy: [xOf(0), Y, xOf(90), Y, xOf(LENGTH), Y], z: [1000, 1000, 1000], anchors: [true, true, true], sidewalks: [0, LEFT] });
        const { rail } = stands(line, stations, 1, (_, y) => 1000 - (y < Y ? 3 : 0));
        expect(at(stations, rail, 40)).toBe(1);
        expect(at(stations, rail, 88)).toBeLessThan(0.5);
        expect(at(stations, rail, 120)).toBe(0);
    });
});

/** The faces of one side of a road on `ground`: one piece, its builder and what stands. */
function built(ground: GroundAt, side: 1 | -1): { b: PieceBuilder } {
    const { line, stations } = road(), b = new PieceBuilder(xOf(100), Y, size(16) / 2);
    const run = { id: 1, line, stations, reach: foldsOf(stations), lift: 0, prio: 0, base: 0, ways: waysOf([line]), far: stations.flatMap(({ along }, j) => (j === 0 || j === stations.length - 1 || Math.floor(along / 10) !== Math.floor(stations[j - 1].along / 10) ? [j] : [])) };
    roadside(run, side, stands(line, stations, side, ground), ground, () => true, () => b);
    return { b };
}

describe('the faces of what stands', () => {
    /** A vertex's metres over the road, its material and its flags. */
    const vertices = (b: PieceBuilder) => {
        const { vertices } = b.piece(0), floats = new Float32Array(vertices), words = new Uint32Array(vertices);
        return (i: number) => ({ z: floats[4 * i + 1] - 1000, mat: words[4 * i + 2] & 127, flags: (words[4 * i + 2] >> 11) & 31 });
    };
    const uniq = (list: number[]) => [...new Set(list)];

    it('stand a wall to its top from one height per station, with the last face of its backfill out of the casters', () => {
        const { b } = built(bump(0, 5), -1), vertex = vertices(b);
        expect(b.near.length).toBeGreaterThan(0);
        expect(uniq(b.near.map(i => vertex(i).mat))).toEqual(expect.arrayContaining([MAT.GROUND, MAT.WALL, MAT.CAP]));
        expect(b.casters.some(i => vertex(i).mat === MAT.WALL)).toBe(true);
        expect(b.casters.every((i, k) => k % 3 > 0 || vertex(i).flags !== CF.MESH)).toBe(true);
        expect(b.far.every((i, k) => k % 3 > 0 || vertex(i).mat === MAT.GROUND)).toBe(true);
        expect(Math.max(...b.casters.map(i => vertex(i).z))).toBeGreaterThan(1.5);
    });

    it('share their vertices across the near, far and caster ranges', () => {
        const { b } = built(bump(0, 5), -1), near = new Set(b.near), piece = b.piece(0);
        expect(b.casters.every(i => near.has(i))).toBe(true);
        expect(piece.casters.first).toBe(piece.far.first + piece.far.count - piece.casters.count);
        expect(piece.casters.count).toBe(b.casters.length);
    });

    it('sink a wall under the road at its ends', () => {
        const { b } = built(bump(0, 5), -1), vertex = vertices(b);
        expect(Math.min(...b.near.map(i => vertex(i).z))).toBeLessThan(-0.6);
    });

    it('run a post to a foot under any fill, and sink a rail\'s beam at its ends', () => {
        const { b } = built(bump(-3, 0), 1), vertex = vertices(b), posts = uniq(b.near).filter(i => vertex(i).mat === MAT.RAIL_POST), feet = Math.min(...posts.map(i => vertex(i).z));
        expect(feet).toBeLessThanOrEqual(-1.5);
        expect(Math.min(...uniq(b.near).filter(i => vertex(i).mat === MAT.RAIL).map(i => vertex(i).z))).toBeLessThan(-1);
    });

    it('stand no delineator beside a rail or a wall, and others on the ground beside the road', () => {
        const { b } = built(bump(0, 5), -1), vertex = vertices(b), posts = uniq(b.near).filter(i => vertex(i).mat === MAT.DELI);
        expect(posts.length).toBeGreaterThan(0);
        const { vertices: bytes, x } = b.piece(0), steps = new Int16Array(bytes), places = posts.map(i => alongOf(x + steps[8 * i] / 3));
        expect(places.every(m => m < 46 || m > 114)).toBe(true);
    });

    it('hold nothing where the ground is unknown', () => {
        const { b } = built(() => NaN, 1);
        expect(b.count).toBe(0);
    });
});

it('zones reach both ways from what they mark', () => {
    const { stations } = road(), zones = zonesOf(stations, j => j === 10);
    expect(zones[10]).toBe(0);
    expect(zones[12]).toBeCloseTo(stations[12].along - stations[10].along);
    expect(zones[8]).toBeCloseTo(stations[10].along - stations[8].along);
    expect(zonesOf(stations, () => false)[0]).toBe(Infinity);
});
