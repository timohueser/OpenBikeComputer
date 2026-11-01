import { describe, expect, it } from 'vitest';
import { SIDE_ON } from '../stream/features';
import { metresPerUnit, PATH, PROFILES, ROAD, zebraTagged, type Line } from '../terrain/bins';
import { LEFT } from '../terrain/town';
import { planOf } from './mesh';
import { lowered, LOOK, MAT, sideOf } from './section';
import { CUT, type Station } from './stations';

const O = { x: 2 ** 28, y: 2 ** 29 }, unit = metresPerUnit(O.y), CLASSES = Object.keys(PROFILES);
const profile = (name: string) => CLASSES.indexOf(name);

/** A line through points in metres east and north of O, straight between them. */
function way(points: number[][], extra: Partial<Line> = {}): Line {
    const count = points.length, zero = new Array(count).fill(0), tangent = points.map((_, v) => {
        const [a, b] = [points[Math.max(0, v - 1)], points[Math.min(count - 1, v + 1)]];
        return Math.atan2(a[1] - b[1], b[0] - a[0]);
    });
    return {
        kind: ROAD, xy: points.flatMap(([e, n]) => [Math.round(O.x + e / unit), Math.round(O.y - n / unit)]), z: new Array(count).fill(1000), h: 3, code: 2, ford: false, profile: profile('tertiary'),
        anchors: new Array(count).fill(true), plan: { tangent, grade: zero, bank: zero, widen: zero }, ...extra,
    };
}
const at = (e: number, n: number) => ({ x: O.x + e / unit, y: O.y - n / unit });
const plan = (lines: Line[], crossings: { x: number; y: number }[] = []) => planOf({ lines, buildings: [], barriers: [], crossings });
const painted = (stations: Station[], mat: number) => stations.filter(({ paint }) => paint?.mat === mat);
const stationsOf = (lines: Line[], line: Line, crossings: { x: number; y: number }[] = []) => plan(lines, crossings).runs.find(run => run.line === line)!.stations;
const main = () => way([[-100, 0], [100, 0]]);

describe('a zebra', () => {
    it('comes from the tags of a crossing node', () => {
        expect(['zebra', 'zebra:double', 'ladder', 'yes'].every(value => zebraTagged({ 'crossing:markings': value }))).toBe(true);
        expect(['marked', 'zebra', 'uncontrolled'].every(crossing => zebraTagged({ crossing }))).toBe(true);
        expect(zebraTagged({ crossing: 'uncontrolled', 'crossing:markings': 'no' })).toBe(false);
        expect(['unmarked', 'traffic_signals', 'no'].some(crossing => zebraTagged({ crossing }))).toBe(false);
        expect(zebraTagged({})).toBe(false);
    });

    it('lies 2 m each way along the road at its node, and nowhere without one', () => {
        const road = main(), list = painted(stationsOf([road], road, [at(10, 1)]), LOOK.ZEBRA), middle = stationsOf([road], road, [at(10, 1)]).find(({ along }) => Math.abs(along - 110) < 0.5);
        expect(list[0].along).toBeCloseTo(108, 1);
        expect(list[list.length - 1].along).toBeLessThan(112);
        expect(middle?.paint?.mat).toBe(LOOK.ZEBRA);
        const bare = main();
        expect(painted(stationsOf([bare], bare), LOOK.ZEBRA)).toEqual([]);
    });

    it('stays off a node far from the road, and off a road of soil', () => {
        const road = main();
        expect(painted(stationsOf([road], road, [at(10, 4)]), LOOK.ZEBRA)).toEqual([]);
        const gravel = way([[-100, 0], [100, 0]], { code: 3 });
        expect(stationsOf([gravel], gravel, [at(10, 0)]).some(({ paint }) => paint)).toBe(false);
    });

    it('takes the material of the surface under it', () => {
        const setts = way([[-100, 0], [100, 0]], { surface: 'sett' });
        expect(painted(stationsOf([setts], setts, [at(0, 0)]), LOOK.ZEBRA_SETT).length).toBeGreaterThan(0);
    });

    it('runs on over the two lines that share its node, and leaves a side arm alone', () => {
        const a = way([[-100, 0], [0, 0]]), b = way([[0, 0], [100, 0]]), arm = way([[0, 0], [0, 100]], { sideWays: [SIDE_ON, 0], profile: profile('residential') });
        const { runs } = plan([a, b, arm], [at(0, 0)]), of = (line: Line) => runs.find(run => run.line === line)!.stations;
        expect(painted(of(a), LOOK.ZEBRA).length).toBeGreaterThan(0);
        expect(painted(of(b), LOOK.ZEBRA).length).toBeGreaterThan(0);
        expect(painted(of(arm), LOOK.ZEBRA)).toEqual([]);
    });

    it('keeps its stripes straight across a bend: its vertices read metres off one straight line through the node', () => {
        const points = Array.from({ length: 31 }, (_, k) => [40 * Math.sin(k * 0.05), 40 * Math.cos(k * 0.05)]);
        const bend = way(points), [e, n] = points[15], list = painted(stationsOf([bend], bend, [at(e, n)]), LOOK.ZEBRA), first = list[0], { across } = first.paint!;
        // The way's middle leaves that line as the road turns, while a point on the line reads the same as the node.
        expect(Math.abs(across(first, first.x, first.y, 0))).toBeGreaterThan(0.02);
        const ahead = at(e + 5 * Math.cos(0.75), n - 5 * Math.sin(0.75)), { x, y } = at(e, n);
        expect(Math.abs(across(first, ahead.x, ahead.y, 0))).toBeLessThan(0.05);
        expect(across(first, x, y, 0)).toBe(0);
    });
});

describe('a lowered kerb', () => {
    const town = () => way([[-100, 0], [100, 0]], { sidewalks: [LEFT], profile: profile('residential') });

    it('falls under a zebra to 2.5 cm over the carriageway and ramps back in 1.2 m, while the sidewalk keeps its level', () => {
        const road = town(), stations = stationsOf([road], road, [at(0, 0)]), middle = stations.find(({ along }) => Math.abs(along - 100) < 1)!;
        expect(middle.drop![0]).toBe(1);
        const full = sideOf(road, 1, middle.v, middle.h).parts, low = lowered(full, 1), walk = low.findIndex(({ mat }) => mat === MAT.SIDEWALK);
        expect(low[3].dz - full[3].dz).toBeCloseTo(-0.095, 5);
        expect(low[walk].dz).toBe(full[walk].dz);
        expect(low.at(-5)!.dz).toBe(full.at(-5)!.dz);
        expect(stations.filter(({ along }) => along > 103.25).every(({ drop }) => !drop![0])).toBe(true);
    });

    it('takes the width of a path that runs on across the road, on the sides with a kerb, and stays whole under one that stops', () => {
        const road = () => way([[-100, 0], [0, 0], [100, 0]], { sidewalks: [LEFT, LEFT], profile: profile('residential') }), path = (end: number[]) => way([[0, 0], end], { kind: PATH, h: 1, code: 8, profile: profile('footway') });
        const on = road(), stations = stationsOf([on, path([0, 60]), path([0, -60])], on);
        expect(stations.find(({ along }) => Math.abs(along - 100) < 0.1)!.drop).toEqual([1, 0]);
        expect(stations.filter(({ along }) => along > 103).every(({ drop }) => !drop?.[0])).toBe(true);
        const stops = road();
        expect(stationsOf([stops, path([0, 60])], stops).some(({ drop }) => drop)).toBe(false);
    });

    it('takes the width of a driveway that crosses the sidewalk', () => {
        const road = way([[-100, 0], [0, 0], [100, 0]], { sidewalks: [LEFT, LEFT], profile: profile('residential') }), drive = way([[0, 0], [0, 60]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2 });
        const stations = stationsOf([road, drive], road);
        expect(stations.find(({ along }) => Math.abs(along - 100) < 0.1)!.drop![0]).toBe(1);
    });
});

describe('a mapped crossing', () => {
    it('that lies on the road to its last point draws nothing there', () => {
        const road = way([[-100, 0], [0, 0], [100, 0]], { sidewalks: [LEFT, LEFT], profile: profile('residential') }), crossing = way([[0, 0], [0, 3.5]], { kind: PATH, h: 1, code: 8, profile: profile('footway') });
        const stations = stationsOf([road, crossing], crossing);
        expect(stations.slice(0, -1).every(({ cut }) => cut === CUT.ALL)).toBe(true);
    });
});

describe('a give-way line', () => {
    const through = (name = 'secondary') => way([[-100, 0], [0, 0], [100, 0]], { profile: profile(name) });
    const stem = (name: string, extra: Partial<Line> = {}, end = [0, 80]) => way([[0, 0], end], { sideWays: [SIDE_ON, 0], profile: profile(name), ...extra });

    it('lies across the mouth of a minor road at a main road, with the triangle in the lane that stops', () => {
        const arm = stem('residential'), stations = stationsOf([through(), arm], arm), list = painted(stations, LOOK.GIVE_WAY);
        expect(list.length).toBeGreaterThan(0);
        const start = list[0].along, paint = list[0].paint!;
        expect(paint.along(list[0], 0)).toBeCloseTo(0, 5);
        expect(stations[stations.indexOf(list[list.length - 1]) + 1].along - start).toBeCloseTo(4.2, 1);
        // The arm leaves its junction along its line, so traffic that comes to it keeps to the line's left.
        expect(paint.across(list[0], 0, 0, 1)).toBe(1);
    });

    it('counts back from the junction on an arm drawn toward it, and flips the lane', () => {
        const arm = way([[0, 80], [0, 0]], { sideWays: [0, 1], profile: profile('residential') }), [first] = painted(stationsOf([through(), arm], arm), LOOK.GIVE_WAY);
        expect(first).toBeDefined();
        expect(first.paint!.across(first, 0, 0, 1)).toBe(-1);
    });

    it('needs a road of a lesser class, on a paved way', () => {
        const peer = stem('secondary'), link = stem('secondary_link');
        expect(painted(stationsOf([through(), peer], peer), LOOK.GIVE_WAY)).toEqual([]);
        expect(painted(stationsOf([through(), link], link), LOOK.GIVE_WAY)).toEqual([]);
        const gravel = stem('residential', { code: 3 });
        expect(painted(stationsOf([through(), gravel], gravel), LOOK.GIVE_WAY)).toEqual([]);
        const quiet = stem('residential');
        expect(painted(stationsOf([through('residential'), quiet], quiet), LOOK.GIVE_WAY)).toEqual([]);
    });

    it('keeps the line alone where the arm has no room for the triangle', () => {
        const arm = stem('residential', {}, [0, 8.5]), stations = stationsOf([through(), arm], arm), list = painted(stations, LOOK.GIVE_WAY);
        expect(list.length).toBe(1);
        expect(stations[stations.indexOf(list[0]) + 1].along - list[0].along).toBeCloseTo(0.3, 1);
    });
});

describe('a roundabout', () => {
    /** A ring of radius 15 m, closed at its east point, drawn clockwise as one way is, with a road at each quarter. */
    const ring = (arms: number[]) => {
        const points = Array.from({ length: 49 }, (_, k) => {
            const t = (-k * 7.5 * Math.PI) / 180;
            return [15 * Math.cos(t), 15 * Math.sin(t)];
        }), line = way(points, { roundabout: true, code: 7, profile: profile('tertiary'), h: 3 });
        const out = arms.map(degrees => {
            const t = (-degrees * Math.PI) / 180, v = degrees / 7.5;
            line.sideWays = line.sideWays ?? new Array(49).fill(0);
            const start = points[v];
            return way([start, [85 * Math.cos(t), 85 * Math.sin(t)]], { sideWays: [SIDE_ON, 0], profile: profile('tertiary') });
        });
        return { line, out };
    };

    it('stops each road that enters it at a line, whatever its class, and the ring runs on with no paint', () => {
        const { line, out } = ring([90, 180, 270]), { runs } = plan([line, ...out]), of = (one: Line) => runs.find(run => run.line === one)!.stations;
        expect(of(line).some(({ paint }) => paint)).toBe(false);
        for (const arm of out) expect(painted(of(arm), LOOK.GIVE_WAY).length).toBeGreaterThan(0);
    });

    it('lets out a one-way road that leaves it, and stops one that enters', () => {
        const { line } = ring([]), leaves = way([[0, -15], [0, -85]], { sideWays: [SIDE_ON, 0], code: 7 }), enters = way([[0, 85], [0, 15]], { sideWays: [0, 1], code: 7 });
        const { runs } = plan([line, leaves, enters]), of = (one: Line) => runs.find(run => run.line === one)!.stations;
        expect(painted(of(leaves), LOOK.GIVE_WAY)).toEqual([]);
        expect(painted(of(enters), LOOK.GIVE_WAY).length).toBeGreaterThan(0);
    });

    it('draws its ring as a one-way road: no middle line, from its surface alone', () => {
        const { line } = ring([]);
        expect(line.roundabout).toBe(true);
        expect(plan([line]).runs[0].stations.some(({ cut }) => cut)).toBe(false);
    });
});
