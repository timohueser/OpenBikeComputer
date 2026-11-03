import { describe, expect, it } from 'vitest';
import { features, metresPerUnit, PROFILES, ROAD, SURFACES, type Barrier, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { roadside } from './details';
import type { GroundAt } from './ground';
import { PieceBuilder } from './piece';
import { CF, LIFT_M, MAT } from './section';
import { roomOf } from './room';
import { barriersOf, standingOf, zonesOf } from './standing';
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
const RESIDENTIAL = Object.keys(PROFILES).indexOf('residential'), ROAD_SURFACE = Object.keys(SURFACES).indexOf('road') + 1;
/** A residential street, its core 2.4 m each side, parallel to the road `out` metres to the south (its right) from `from` to `to` metres along, `rise` metres over it. */
const street = (from: number, to: number, out: number, rise: number): Line => ({
    kind: ROAD, xy: [xOf(from), Y + out / unit, xOf(to), Y + out / unit], z: [1000 + rise, 1000 + rise], h: 2, code: ROAD_SURFACE, ford: false, profile: RESIDENTIAL, anchors: [true, true],
});
/** The road's right core edge lies 3.6 m out: a street 1 m past it, 5 m over the road, leaves its cut no room. */
const UPPER_OUT_M = 3.6 + 1 + 2.4, UPPER_RISE_M = 5;
const upper = (from = 60, to = 100) => street(from, to, UPPER_OUT_M, UPPER_RISE_M);
/** A level residential street that leaves the road southward `at` metres along: it meets the road's right side. */
const cross = (at: number): Line => ({ ...street(0, 0, 0, 0), xy: [xOf(at), Y, xOf(at), Y + 30 / unit] });
const stands = (line: Line, stations: Station[], side: 1 | -1, ground: GroundAt, mouths: Float64Array = far(stations), towns = mouths, mapped: Barrier[] = [], others: Line[] = []) => standingOf(line, stations, foldsOf(stations), side, ground, mouths, towns, barriersOf(mapped), waysOf([line, ...others]));
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

    it('stands no wall on an open hillside: the cone of its cut meets the ground before its reach', () => {
        const { line, stations } = road();
        for (const slope of [0.5, 1, 2]) {
            const { wall } = stands(line, stations, -1, (_, y) => 1000 + Math.max(0, y - Y) * unit * slope);
            expect(wall.every(share => share === 0)).toBe(true);
        }
        expect(stands(line, stations, -1, bump(0, 5)).wall.every(share => share === 0)).toBe(true);
    });

    it('stands a wall where the next way beside the road lies higher than the cones of the two reach, as high as that way', () => {
        const { line, stations } = road(), { rail, wall, high } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [upper()]);
        expect(at(stations, wall, 80)).toBe(1);
        expect(at(stations, wall, 50)).toBeGreaterThan(0);
        expect(at(stations, wall, 50)).toBeLessThan(1);
        expect(at(stations, wall, 40)).toBe(0);
        expect(at(stations, high, 80)).toBeCloseTo(UPPER_RISE_M, 3);
        expect(rail.every(share => share === 0)).toBe(true);
        // Its other side stands none, nor the upper street's side down to the road.
        expect(stands(line, stations, 1, () => 1000, undefined, undefined, undefined, [upper()]).wall.every(share => share === 0)).toBe(true);
        const top = road({ ...upper(0, LENGTH), profile: Object.keys(PROFILES).indexOf('secondary') });
        const down = standingOf(top.line, top.stations, foldsOf(top.stations), 1, () => 1000, far(top.stations), far(top.stations), barriersOf([]), waysOf([top.line, line]));
        expect(down.wall.every(share => share === 0)).toBe(true);
    });

    it('stands no wall where the next way lies low enough for the cones to meet', () => {
        const { line, stations } = road();
        // 1 m out, a cut and a fill of 0.85 meet over 0.94 m. 0.1 m out, 0.5 m is less than the least wall.
        for (const [out, rise] of [[1, 0.9], [3, 3.5], [8, 5], [0.1, 0.5]]) {
            const { wall } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [street(60, 100, 3.6 + out + 2.4, rise)]);
            expect(wall.every(share => share === 0)).toBe(true);
        }
    });

    it('stands no wall for a way that runs across the side, as a bridge\'s approach does: its abutment holds that ground', () => {
        const { line, stations } = road(), approach: Line = { ...street(0, 0, 0, UPPER_RISE_M), xy: [xOf(80), Y + UPPER_OUT_M / unit, xOf(80), Y + 40 / unit] };
        expect(stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [approach]).wall.every(share => share === 0)).toBe(true);
    });

    it('stands a wall between two hairpin legs 6 m apart on a 1:1 slope, up to the upper leg', () => {
        // Two residential legs, their cores 2.4 m each side: 1.2 m apart. The upper lies 6 m south, on the hillside's level there.
        const hill: GroundAt = (_, y) => 1000 + (y - Y) * unit, lower = road({ profile: RESIDENTIAL, h: 2, code: ROAD_SURFACE }), leg = street(0, LENGTH, 6, 6);
        const { wall, high } = standingOf(lower.line, lower.stations, foldsOf(lower.stations), -1, hill, far(lower.stations), far(lower.stations), barriersOf([]), waysOf([lower.line, leg]));
        expect(at(lower.stations, wall, 100)).toBe(1);
        expect(at(lower.stations, high, 100)).toBeCloseTo(6, 3);
    });

    it('stands a wall on the lower leg of a hairpin of one line whose legs climb apart, as high as the upper leg, and none in its tip', () => {
        // East 80 m, a hairpin of 3 m radius to the right, and back west 80 m, climbing 8 % all along.
        const points: [number, number, number][] = [];
        for (let m = 0; m < 80; m += 2) points.push([m, 0, 0]);
        for (let k = 0; k < 12; k++) points.push([80 + 3 * Math.sin(Math.PI * k / 12), 3 - 3 * Math.cos(Math.PI * k / 12), Math.PI * k / 12]);
        for (let m = 80; m >= 0; m -= 2) points.push([m, 6, Math.PI]);
        const xy = points.flatMap(([x, y]) => [xOf(x), Y + y / unit]), tangent = points.map(([, , t]) => t), metres = points.map((_, i) => (i ? Math.hypot(points[i][0] - points[i - 1][0], points[i][1] - points[i - 1][1]) : 0));
        const zero = tangent.map(() => 0), z = metres.map((_, i) => 1000 + 0.08 * metres.slice(0, i + 1).reduce((a, b) => a + b, 0));
        const { line, stations } = road({ xy, z, h: 2, code: ROAD_SURFACE, profile: RESIDENTIAL, anchors: tangent.map(() => true), plan: { tangent, grade: zero.map(() => 0.08), bank: zero, widen: zero } });
        const { wall, high } = stands(line, stations, -1, () => NaN), length = stations[stations.length - 1].along;
        // 40 m before the tip the legs lie 7.1 m apart in height: the wall holds the upper leg there.
        const lowerAt = stations.findIndex(s => s.along >= 40), upperAt = stations.findIndex(s => s.along >= length - 40);
        expect(wall[lowerAt]).toBe(1);
        expect(high[lowerAt]).toBeCloseTo(stations[upperAt].z - stations[lowerAt].z, 0);
        expect(wall[upperAt]).toBe(0);
        // Inside the tip the side folds short of a wall's backfill: no wall.
        expect(wall[stations.findIndex(s => s.along >= 80 + Math.PI * 1.5)]).toBe(0);
    });

    it('stands a wall at the core\'s edge, behind a sidewalk, and ramps it down where the sidewalk starts, as its edge steps', () => {
        const { line, stations } = road({ xy: [xOf(0), Y, xOf(90), Y, xOf(LENGTH), Y], z: [1000, 1000, 1000], anchors: [true, true, true], sidewalks: [0, RIGHT] });
        const others = [street(20, 180, 3 + 2 + 1 + 2.4, UPPER_RISE_M)], { wall, high } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, others);
        expect(at(stations, wall, 50)).toBe(1);
        expect(at(stations, wall, 88)).toBeLessThan(0.5);
        expect(at(stations, wall, 130)).toBe(1);
        expect(at(stations, high, 130)).toBeCloseTo(UPPER_RISE_M, 3);
    });

    it('lets its top fall no steeper than the cut where the ground that it holds steps, and never over that ground', () => {
        // The next way lies 1 m out up to 100 m along, and 4 m out past it: the ground held 2.2 m out falls 2.6 m there.
        const { line, stations } = road(), others = [street(40, 100, 3.6 + 1 + 2.4, 6), street(100, 160, 3.6 + 4 + 2.4, 6)];
        const { wall, high } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, others);
        const held = stations.map(s => roomOf(line, s, -1, () => 1000, waysOf([line, ...others])).held);
        for (let j = 1; j < stations.length; j++) {
            if (wall[j] <= 0 || wall[j - 1] <= 0) continue;
            expect(Math.abs(high[j] - high[j - 1])).toBeLessThanOrEqual(0.85 * (stations[j].along - stations[j - 1].along) + 1e-9);
            expect(high[j]).toBeLessThanOrEqual(held[j] + 1e-9);
        }
        expect(Math.max(...held) - Math.min(...held.filter((_, j) => wall[j] >= 1))).toBeGreaterThan(1);
    });

    it('holds the stamped ground at the back of its backfill, with no step as the next way moves out past it', () => {
        const { line, stations } = road(), s = stations[stations.findIndex(st => st.along >= 80)];
        let last: number | undefined;
        for (let out = 0.2; out <= 4; out += 0.1) {
            const { open, held } = roomOf(line, s, -1, () => 1000, waysOf([line, street(60, 100, 3.6 + out + 2.4, 6)]));
            // The ramp of the stamp falls about 2.4 m per metre there: no step of the 3.6 m between the way's level and the cut alone.
            if (last !== undefined) expect(Math.abs(held - last)).toBeLessThan(0.3);
            // Up to the backfill's back the next way's level, past it under that level, and over what the cut alone holds.
            if (out < 2.05) expect(held).toBeCloseTo(6, 1);
            if (out > 2.35) expect(held).toBeLessThan(6);
            if (!open) expect(held).toBeGreaterThan(0.02 + 0.85 * 2.2 * 10 / 7.8);
            last = held;
        }
    });

    it('holds the section\'s bank past a line\'s end, so two near stations make no drop out of it', () => {
        const { line, stations } = road(), last = stations.length - 1;
        stations[last - 1].along = stations[last].along - 2;
        stations[last].bank = 0.2;
        expect(stands(line, stations, 1, () => 1000).rail.every(share => share === 0)).toBe(true);
    });

    /** Per stretch of a field that stands, how often its share rises again after it fell. */
    const dipsOf = (shares: Float64Array) => {
        const runs: number[] = [];
        let falling = false;
        shares.forEach((share, j) => {
            const last = shares[j - 1] ?? 0;
            if (share > 0 && last === 0) {
                runs.push(0);
                falling = false;
            }
            if (share > last && falling) runs[runs.length - 1]++;
            falling ||= share < last;
        });
        return runs;
    };

    it.each([[-3, 0, 1, 'rail', 28]] as const)('closes a gap of up to two ramps and a break whole, and opens a wider one to nothing, with one rise and one fall to a stretch', (left, right, side, field, width) => {
        const { line, stations } = road();
        for (let gap = 2; gap <= 50; gap += 2) {
            const off = (x: number) => Math.abs(alongOf(x) - 100);
            const standing = stands(line, stations, side, (x, y) => 1000 + (off(x) >= gap / 2 && off(x) <= gap / 2 + 25 ? (y < Y ? left : right) : 0));
            const dips = dipsOf(standing[field]);
            if (gap <= width - 4) expect(dips).toEqual([0]);
            else if (gap >= width + 2) expect(dips).toEqual([0, 0]);
            else expect(dips.every(count => count === 0)).toBe(true);
            if (gap <= width - 4) expect(at(stations, standing[field], 100)).toBe(1);
        }
    });

    it('closes a gap between two walls narrower than two ramps and a break, and opens a wider one to nothing', () => {
        const { line, stations } = road();
        const narrow = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [upper(40, 90), upper(100, 150)]).wall;
        expect(dipsOf(narrow)).toEqual([0]);
        expect(at(stations, narrow, 95)).toBe(1);
        const wide = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [upper(10, 60), upper(130, 180)]).wall;
        expect(dipsOf(wide)).toEqual([0, 0]);
        expect(at(stations, wide, 95)).toBe(0);
    });

    it('leaves no stub between two ways that meet its side', () => {
        const { line, stations } = road(), { wall } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [upper(60, 120), cross(85), cross(95)]);
        expect(at(stations, wall, 90)).toBe(0);
    });

    it('stops and starts nothing but through its ramp, up to a way that meets its side', () => {
        const { line, stations } = road(), { wall } = stands(line, stations, -1, () => 1000, undefined, undefined, undefined, [upper(60, 120), cross(100)]);
        expect(at(stations, wall, 80)).toBe(1);
        expect(at(stations, wall, 96)).toBeLessThan(0.5);
        expect(at(stations, wall, 99)).toBe(0);
    });

    it('runs on past a junction on its other side: a mouth stops a wall only where a way meets its side', () => {
        const { line, stations } = road(), mouths = zonesOf(stations, j => Math.abs(stations[j].along - 80) < 8);
        const north: Line = { ...cross(80), xy: [xOf(80), Y, xOf(80), Y - 30 / unit] }, { wall } = stands(line, stations, -1, () => 1000, mouths, undefined, undefined, [upper(), north]);
        expect(at(stations, wall, 80)).toBe(1);
    });

    it('takes a mapped barrier first: a rail on level ground, a wall where the ground climbs less than a cut', () => {
        const { line, stations } = road(), north = Y - 4.5 / unit;
        const rail: Barrier = { xy: [xOf(80), north, xOf(120), north], wall: false };
        expect(at(stations, stands(line, stations, 1, bump(0, 0), undefined, undefined, [rail]).rail, 100)).toBe(1);
        const wall: Barrier = { xy: [xOf(70), north, xOf(90), north], wall: true };
        const climbs = stands(line, stations, 1, bump(1, 0), undefined, undefined, [wall]);
        expect(at(stations, climbs.wall, 80)).toBe(1);
        // As high as the ground that it holds.
        expect(at(stations, climbs.high, 80)).toBeCloseTo(1, 3);
    });

    it('stands a mapped wall only where the ground climbs: on the low side it holds the road up, and the drop test stands', () => {
        const { line, stations } = road(), north = Y - 4.5 / unit, wall: Barrier = { xy: [xOf(70), north, xOf(90), north], wall: true };
        const { wall: shares, rail } = stands(line, stations, 1, bump(-6, 0), undefined, undefined, [wall]);
        expect(shares.every(share => share === 0)).toBe(true);
        expect(at(stations, rail, 80)).toBe(1);
    });

    it('stands a mapped rail at any class and in a town, and out of a mouth', () => {
        const { line, stations } = road({ profile: Object.keys(PROFILES).indexOf('service') }), north = Y - 4.5 / unit, rail: Barrier = { xy: [xOf(80), north, xOf(120), north], wall: false };
        const town = zonesOf(stations, j => stations[j].along > 60 && stations[j].along < 140);
        expect(at(stations, stands(line, stations, 1, bump(0, 0), far(stations), town, [rail]).rail, 100)).toBe(1);
        const mouth = zonesOf(stations, j => Math.abs(stations[j].along - 100) < 8);
        const { rail: split } = stands(line, stations, 1, bump(0, 0), mouth, mouth, [rail]);
        expect(at(stations, split, 100)).toBe(0);
        expect(at(stations, split, 86)).toBeGreaterThan(0);
    });

    it('keeps a mapped rail off a sidewalk: its beam would stand on it', () => {
        const { line, stations } = road({ xy: [xOf(0), Y, xOf(90), Y, xOf(LENGTH), Y], z: [1000, 1000, 1000], anchors: [true, true, true], sidewalks: [0, LEFT] }), north = Y - 4.5 / unit;
        const { rail } = stands(line, stations, 1, bump(0, 0), undefined, undefined, [{ xy: [xOf(40), north, xOf(160), north], wall: false }]);
        expect(at(stations, rail, 60)).toBe(1);
        expect(at(stations, rail, 120)).toBe(0);
    });

    it('reads the barriers of a tile from its features', () => {
        const part = { outer: false, xy: new Int32Array([100, 100, 200, 100]), z: null };
        const { barriers } = features([{ kind: 'guard-rail', tags: {}, geometry: 'line', parts: [part] }, { kind: 'retaining-wall', tags: { height: '2.5' }, geometry: 'line', parts: [part] }], { level: 13, x: 4286, y: 2888 });
        expect(barriers.map(({ wall }) => wall)).toEqual([false, true]);
        expect(barriers[0].xy).toHaveLength(4);
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
function built(ground: GroundAt, side: 1 | -1, others: Line[] = []): { b: PieceBuilder } {
    const { line, stations } = road(), b = new PieceBuilder(xOf(100), Y);
    const run = { id: 1, line, stations, reach: foldsOf(stations), lift: 0, base: 0, ways: waysOf([line]), far: stations.flatMap(({ along }, j) => (j === 0 || j === stations.length - 1 || Math.floor(along / 10) !== Math.floor(stations[j - 1].along / 10) ? [j] : [])) };
    roadside(run, side, stands(line, stations, side, ground, undefined, undefined, undefined, others), () => true, () => b);
    return { b };
}

describe('the faces of what stands', () => {
    /** A vertex's metres over the road, its material and its flags. */
    const vertices = (b: PieceBuilder) => {
        const { vertices } = b.piece(0), floats = new Float32Array(vertices), words = new Uint32Array(vertices);
        return (i: number) => ({ z: floats[4 * i + 1] - 1000, mat: words[4 * i + 2] & 127, flags: (words[4 * i + 2] >> 11) & 31, across: ((words[4 * i + 3] << 20) >> 20) * 16 / 2047, along: (words[4 * i + 3] >>> 12) / 64 });
    };
    const uniq = (list: number[]) => [...new Set(list)];

    it('stand a wall to its top from one height per station, with the last face of its backfill out of the casters', () => {
        const { b } = built(() => 1000, -1, [upper()]), vertex = vertices(b);
        expect(b.near.length).toBeGreaterThan(0);
        expect(uniq(b.near.map(i => vertex(i).mat))).toEqual(expect.arrayContaining([MAT.GROUND, MAT.WALL, MAT.CAP]));
        expect(b.casters.some(i => vertex(i).mat === MAT.WALL)).toBe(true);
        expect(b.casters.every((i, k) => k % 3 > 0 || vertex(i).flags !== CF.MESH)).toBe(true);
        expect(b.far.every((i, k) => k % 3 > 0 || vertex(i).mat === MAT.GROUND)).toBe(true);
        expect(Math.max(...b.casters.map(i => vertex(i).z))).toBeGreaterThan(1.5);
    });

    it('share their vertices across the near, far and caster ranges', () => {
        const { b } = built(() => 1000, -1, [upper()]), near = new Set(b.near), piece = b.piece(0);
        expect(b.casters.every(i => near.has(i))).toBe(true);
        expect(piece.casters.first).toBe(piece.far.first + piece.far.count - piece.casters.count);
        expect(piece.casters.count).toBe(b.casters.length);
    });

    it('sink a wall under the road at its ends', () => {
        const { b } = built(() => 1000, -1, [upper()]), vertex = vertices(b);
        expect(Math.min(...b.near.map(i => vertex(i).z))).toBeLessThan(-0.6);
    });

    it('end a wall\'s capstone short of the next way\'s core, over which it would draw, and its top on that way\'s level', () => {
        const { b } = built(() => 1000, -1, [street(60, 100, 3.6 + 0.2 + 2.4, UPPER_RISE_M)]), vertex = vertices(b), caps = uniq(b.near).map(vertex).filter(({ mat, along }) => mat === MAT.CAP && along > 62 && along < 98);
        expect(caps.length).toBeGreaterThan(0);
        expect(Math.max(...caps.map(({ across }) => -across))).toBeLessThan(3.6 + 0.2 + 0.01);
        expect(Math.max(...caps.map(({ z }) => z))).toBeCloseTo(UPPER_RISE_M + LIFT_M, 3);
    });

    it('run a post to a foot under any fill, and sink a rail\'s beam at its ends', () => {
        const { b } = built(bump(-3, 0), 1), vertex = vertices(b), posts = uniq(b.near).filter(i => vertex(i).mat === MAT.RAIL_POST), feet = Math.min(...posts.map(i => vertex(i).z));
        expect(feet).toBeLessThanOrEqual(-1.5);
        expect(Math.min(...uniq(b.near).filter(i => vertex(i).mat === MAT.RAIL).map(i => vertex(i).z))).toBeLessThan(-1);
    });

    it('stand no delineator beside a rail or a wall, and others on the ground beside the road', () => {
        const { b } = built(() => 1000, -1, [upper()]), vertex = vertices(b), posts = uniq(b.near).filter(i => vertex(i).mat === MAT.DELI);
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
