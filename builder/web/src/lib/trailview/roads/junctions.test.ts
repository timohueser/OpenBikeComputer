import { describe, expect, it } from 'vitest';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { metresPerUnit, PATH, PROFILES, RAIL, RAIL_CODE, ROAD, type Line } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { cornerFaces, mouthFaces } from './junction-faces';
import { planOf } from './mesh';
import { PieceBuilder, ROAD_VERTEX } from './piece';
import { CF, LOOK } from './section';
import { CUT, type Station } from './stations';
import { sweep } from './sweep';

const O = { x: 2 ** 28, y: 2 ** 29 }, unit = metresPerUnit(O.y), CLASSES = Object.keys(PROFILES);
const profile = (name: string) => CLASSES.indexOf(name);

/** Options of a road, by the vertices of its points: the bake's side marks; the rest are fields of the line. */
type Extra = Partial<Line> & { bank?: number[]; widen?: number[]; grade?: number[] };

/** A line through points in metres east and north of O, straight between them, with tangents along its chords. */
function way(points: number[][], { bank, widen, grade, ...extra }: Extra = {}): Line {
    const xy = points.flatMap(([e, n]) => [Math.round(O.x + e / unit), Math.round(O.y - n / unit)]), count = points.length, zero = new Array(count).fill(0);
    const tangent = points.map((_, v) => {
        const [a, b] = [points[Math.max(0, v - 1)], points[Math.min(count - 1, v + 1)]];
        return Math.atan2(a[1] - b[1], b[0] - a[0]);
    });
    return {
        kind: ROAD, xy, z: new Array(count).fill(1000), h: 3, code: 2, ford: false, profile: profile('tertiary'), anchors: new Array(count).fill(true),
        plan: { tangent, grade: grade ?? zero, bank: bank ?? zero, widen: widen ?? zero }, ...extra,
    };
}

/** A curve by its centre, radius and angles (degrees from east toward north), in points about `step` degrees apart. */
function arc([e, n]: number[], r: number, from: number, to: number, step = 6): number[][] {
    const count = Math.ceil(Math.abs(to - from) / step);
    return Array.from({ length: count + 1 }, (_, k) => {
        const t = (from + ((to - from) * k) / count) * Math.PI / 180;
        return [e + r * Math.cos(t), n + r * Math.sin(t)];
    });
}

const metresOf = ({ x, y }: { x: number; y: number }) => [(x - O.x) * unit, (O.y - y) * unit];

/** The stations of a line with `bits` cut, as a span of metres from its start. */
function cuts(stations: Station[], bits: number): [number, number] | undefined {
    const cut = stations.filter((s, j) => j + 1 < stations.length && ((s.cut ?? 0) & bits) === bits);
    if (!cut.length) return undefined;
    return [cut[0].along, stations[stations.indexOf(cut[cut.length - 1]) + 1].along];
}

/** The plan of lines, and the stations of each. */
function planned(lines: Line[]) {
    const plan = planOf(lines), of = (line: Line) => plan.runs.find(run => run.line === line)!;
    return { ...plan, of, stations: (line: Line) => of(line).stations };
}

/** The vertices of a plan's faces at the junction, as a test reads them. */
function faces(plan: ReturnType<typeof planned>) {
    const b = new PieceBuilder(O.x, O.y, 2 ** 13);
    for (const corner of plan.junctions.corners) cornerFaces(b, corner);
    for (const mouth of plan.junctions.mouths) mouthFaces(b, mouth);
    const words = new Uint32Array(b.piece(0).vertices), floats = new Float32Array(b.piece(0).vertices);
    return { b, vertices: Array.from({ length: words.length * 4 / ROAD_VERTEX }, (_, i) => ({ z: floats[4 * i + 1], mat: words[4 * i + 2] & 127, code: (words[4 * i + 2] >> 7) & 15, flags: (words[4 * i + 2] >> 11) & 31 })) };
}

const through = (extra: Extra = {}) => way([[-100, 0], [0, 0], [100, 0]], extra);
const stem = (end: number[], extra: Extra = {}) => way([[0, 0], end], { sideWays: [SIDE_ON, 0], profile: profile('residential'), ...extra });

describe('a T', () => {
    const main = through(), side_ = stem([0, 100]), plan = planned([main, side_]);

    it('has two corners and a mouth, of the radius of the stamp\'s arcs', () => {
        expect(plan.junctions.corners.map(({ r }) => r)).toEqual([expect.closeTo(5, 1), expect.closeTo(5, 1)]);
        expect(plan.junctions.mouths).toHaveLength(1);
    });

    it('cuts the through road\'s side toward the arm from arc end to arc end, and only that', () => {
        const left = cuts(plan.stations(main), CUT.LEFT)!;
        expect(left[1] - left[0]).toBeCloseTo(2 * (3 + 5), 0);
        expect(cuts(plan.stations(main), CUT.RIGHT)).toBeUndefined();
        expect(cuts(plan.stations(main), CUT.ROAD)).toBeUndefined();
    });

    it('starts the arm\'s carriageway on the through road\'s edge, and its sides at the arcs\' ends', () => {
        expect(cuts(plan.stations(side_), CUT.ROAD)).toEqual([0, expect.closeTo(3, 2)]);
        expect(cuts(plan.stations(side_), CUT.LEFT)![1]).toBeCloseTo(3 + 5, 0);
        expect(cuts(plan.stations(side_), CUT.RIGHT)![1]).toBeCloseTo(3 + 5, 0);
    });

    it('stops the through road\'s edge line at the mouth: the carriageway\'s faces there carry the flag on their first vertex', () => {
        const b = new PieceBuilder(O.x, O.y, 2 ** 13), run = plan.of(main), j = run.stations.findIndex(s => s.cut);
        sweep(b, b.near, run, j, j + 1, false);
        const flags = new Uint32Array(b.piece(0).vertices), first = (t: number) => (flags[4 * b.near[t] + 2] >> 11) & 31;
        expect(first(0) & CF.NO_LEFT_LINE).toBe(CF.NO_LEFT_LINE);
        expect(first(0) & CF.NO_RIGHT_LINE).toBe(0);
        const before = new PieceBuilder(O.x, O.y, 2 ** 13);
        sweep(before, before.near, run, 0, 1, false);
        expect((new Uint32Array(before.piece(0).vertices)[4 * before.near[0] + 2] >> 11) & 31).toBe(0);
    });

    it('fills the corners and the mouth in the through road\'s surface', () => {
        const { vertices } = faces(plan), fillet = vertices.filter(({ mat }) => mat === LOOK.ASPHALT);
        expect(fillet.length).toBeGreaterThan(0);
        expect(new Set(fillet.map(({ code }) => code))).toEqual(new Set([main.code]));
    });

    it('keeps its far range on the plan: each cut station, and the faces', () => {
        const { far, stations } = plan.of(main);
        stations.forEach((s, j) => s.cut && expect(far).toContain(j));
        const { b } = faces(plan);
        expect(b.far.length).toBeGreaterThan(0);
        expect(b.far.length).toBeLessThan(b.near.length);
    });
});

describe('a corner that the stamp does not round', () => {
    it('takes a rural road\'s radius: past the stamp\'s widest turn, up to a side arm\'s', () => {
        // 152 degrees between the arm and the road's east way, and 28 between it and the west way.
        const plan = planned([through(), stem([-88, 47])]);
        expect(plan.junctions.corners.map(({ r }) => r).sort((a, b) => b - a)[0]).toBe(7);
    });
});

describe('an X', () => {
    it('has four corners and two mouths', () => {
        const main = through(), a = stem([0, 100]), b = stem([0, -100]), plan = planned([main, a, b]);
        expect(plan.junctions.corners).toHaveLength(4);
        expect(plan.junctions.mouths).toHaveLength(2);
        expect(cuts(plan.stations(main), CUT.LEFT)).toBeDefined();
        expect(cuts(plan.stations(main), CUT.RIGHT)).toBeDefined();
    });
});

describe('a Y', () => {
    it('rounds the through pair\'s inner bend where two lines meet, and leaves one line\'s bend alone', () => {
        const a = way([[-100, 0], [0, 0]]), b = way([[0, 0], [86, 50]]), arm = stem([50, -86]), split = planned([a, b, arm]);
        expect(split.junctions.corners).toHaveLength(3);
        const line = way([[-100, 0], [0, 0], [86, 50]]), one = planned([line, stem([50, -86])]);
        expect(one.junctions.corners).toHaveLength(2);
    });
});

describe('an acute side arm', () => {
    it('meets the edge far along, in a mouth that holds the arm until both its edges have met it', () => {
        const main = through(), arm = stem([90, 42]), plan = planned([main, arm]), [mouth] = plan.junctions.mouths;
        expect(mouth).toBeDefined();
        // 25 degrees off the road, half-widths 3 and 3: the sharp side meets 3 / tan(12.5°) = 13.5 m along.
        expect(mouth.far).toBeCloseTo(13.5, 0);
        for (const { r } of plan.junctions.corners) expect(r === 0 || r >= 0.8).toBe(true);
    });
});

describe('a side arm', () => {
    const banked = (bank: number) => {
        const main = through({ bank: [bank, bank, bank] }), arm = stem([0, 100]), plan = planned([main, arm]);
        const stations = plan.stations(arm), at = (metres: number) => stations.reduce((a, s) => (Math.abs(s.along - metres) < Math.abs(a.along - metres) ? s : a));
        return { main, arm, plan, stations, at };
    };

    it('lies on the through road\'s banked plane at its mouth, and fades to its own heights over 10 m', () => {
        const { at, plan, main } = banked(0.06), lift = plan.of(main).lift - plan.of(plan.junctions.mouths[0].way.line).lift;
        expect(at(3).z).toBeCloseTo(1000 + 0.06 * 3 + lift, 2);
        expect(at(8).z).toBeLessThan(at(3).z);
        expect(at(8).z).toBeGreaterThan(1000);
        expect(at(13.5).z).toBeCloseTo(1000, 3);
    });

    it('never sinks: the through road\'s lower edge rises instead, at most 0.3 m, turning about its other edge, and back over 14 m', () => {
        const { at, plan, main } = banked(-0.06), stations = plan.stations(main), j = stations.findIndex(s => s.along >= 100);
        expect(at(3).z).toBeCloseTo(1000, 3);
        const edge = (s: Station, sign: number) => s.z + s.bank * sign * s.h;
        expect(edge(stations[j], 1)).toBeCloseTo(1000 - 0.18 + 0.18 + plan.of(plan.junctions.mouths[0].way.line).lift - plan.of(main).lift, 2);
        expect(edge(stations[j], -1)).toBeCloseTo(1000 + 0.18, 2);
        expect(stations[0].z).toBe(1000);
        const far = stations.find(s => s.along >= 114.5)!;
        expect(far.bank).toBeCloseTo(-0.06, 3);
    });
});

describe('a track', () => {
    const gravel = 3, dirt = 4;

    it('rounds its corners at 4 m in loose gravel, with no asphalt grain', () => {
        const main = through({ code: gravel, profile: profile('track') }), arm = stem([0, 100], { code: gravel, profile: profile('track') }), plan = planned([main, arm]);
        expect(plan.junctions.corners.map(({ r }) => r)).toEqual([4, 4]);
        expect(faces(plan).vertices.filter(({ mat }) => mat === LOOK.ASPHALT)).toEqual([]);
        expect(faces(plan).vertices.filter(({ mat }) => mat === LOOK.GRAVEL).length).toBeGreaterThan(0);
    });

    it('of soil crosses another with no faces: its ruts run on', () => {
        const main = through({ code: dirt, profile: profile('track') }), arm = stem([0, 100], { code: dirt, profile: profile('track') }), plan = planned([main, arm]);
        expect(plan.junctions.corners).toEqual([]);
        expect(plan.junctions.mouths).toEqual([]);
        expect(plan.stations(main).some(s => s.cut)).toBe(false);
    });

    it('that joins at a hairpin\'s apex leaves the road through: a better class is no side', () => {
        // The bake pairs the track with the road's first way, so the road's second way carries the mark.
        const bend = arc([0, 12], 12, -180, 0), road = way(bend, { sideWays: bend.map((_, v) => (v === 15 ? SIDE_ON : 0)) });
        const track = stem([0, -60], { profile: profile('track'), code: gravel }), plan = planned([road, track]);
        expect(plan.junctions.mouths.map(({ way: { line } }) => line)).toEqual([track]);
        expect(cuts(plan.stations(road), CUT.ROAD)).toBeUndefined();
        expect(plan.junctions.corners).toHaveLength(2);
    });
});

describe('a kerbed corner', () => {
    const town = { sidewalks: [LEFT | RIGHT, LEFT | RIGHT], profile: profile('residential') };

    it('takes a town\'s radius', () => {
        const plan = planned([through(town), stem([0, 100], { sidewalks: [LEFT | RIGHT] })]);
        expect(plan.junctions.corners.map(({ r }) => r)).toEqual([expect.closeTo(5, 1), expect.closeTo(5, 1)]);
    });

    it('keeps an arc wider than its sidewalk where the ways have no room for one, so two sidewalks do not overlap', () => {
        // The next junction 6 m on leaves a corner under 3 m of room.
        const main = through(town), arm = way([[0, 0], [0, 6], [0, 100]], { sidewalks: [LEFT | RIGHT, LEFT | RIGHT], sideWays: [SIDE_ON, 0, 0], profile: profile('residential') });
        const plan = planned([main, arm, way([[0, 6], [80, 6]], { sideWays: [0, 0] })]);
        const walk = Object.values(PROFILES)[profile('residential')].walk;
        for (const { r } of plan.junctions.corners) expect(r).toBeGreaterThanOrEqual(walk + 0.6);
    });

    it('turns a side road that curves away near the junction with no S-bend: the arc leaves its edge along the edge', () => {
        const bend = [[0, 0], ...arc([30, 6], 30, 180, 120).slice(1)], arm = way(bend, { sideWays: bend.map((_, v) => (v ? 0 : SIDE_ON)), profile: profile('residential') });
        const plan = planned([through(), arm]);
        for (const corner of plan.junctions.corners) {
            const { o, r, q, ends } = corner, s = plan.stations(arm).find(({ along }) => Math.abs(along - ends[1]) < 0.02);
            if (q.line !== arm || !s) continue;
            // The arc's radius runs along the section at the arc's end: the two meet square, whichever way the edge bends.
            const foot = metresOf({ x: s.x + s.ax * corner.sq * s.h, y: s.y + s.ay * corner.sq * s.h }), m = metresOf(O);
            expect(Math.hypot(foot[0] - m[0] - o[0], foot[1] - m[1] + o[1])).toBeCloseTo(r, 0);
        }
        expect(plan.junctions.mouths).toHaveLength(1);
    });
});

describe('a way that stops at a road', () => {
    const lane = (end = [0, 60]) => way([[0, 0], end], { kind: PATH, h: 1, code: 6, profile: profile('path') });

    it('as a path ends at the road\'s outer edge, past its shoulder, and its tread meets the shoulder', () => {
        const main = through(), path = lane(), plan = planned([main, path]), s = plan.stations(path)[0], t = plan.stations(path).find(({ cut }) => !cut)!, road = plan.stations(main)[0];
        expect(s.cut).toBe(CUT.ALL);
        // The half-width and the verge of a road code.
        expect(t.along).toBeCloseTo(3 + 0.4, 0);
        expect(t.z).toBeCloseTo(road.z + plan.of(main).lift - 0.02 - plan.of(path).lift, 3);
    });

    it('on a sidewalk ends past it, level with it', () => {
        const main = through({ sidewalks: [LEFT | RIGHT, LEFT | RIGHT], profile: profile('residential') }), path = lane(), plan = planned([main, path]), t = plan.stations(path).find(({ cut }) => !cut)!;
        expect(t.along).toBeCloseTo(3 + Object.values(PROFILES)[profile('residential')].walk, 0);
        expect(t.z).toBeCloseTo(1000 + plan.of(main).lift + 0.12 - plan.of(path).lift, 3);
    });

    it('through a fillet ends past the arc', () => {
        const main = through(), side_ = stem([0, 100]), path = lane([50, 50]), plan = planned([main, side_, path]), t = plan.stations(path).find(({ cut }) => !cut)!;
        // The arc about (8, 8) of 5 m crosses the diagonal 6.3 m out; the path's edges leave it later.
        expect(t.along).toBeGreaterThan(6.3);
        expect(t.along).toBeLessThan(9);
    });

    it('as a driveway on a kerbed road ends at its outer edge too, with no corner', () => {
        const main = through({ sidewalks: [LEFT, LEFT], profile: profile('residential') }), drive = stem([0, 60], { profile: profile('service') }), plan = planned([main, drive]);
        expect(plan.junctions.corners).toEqual([]);
        expect(plan.stations(drive)[0].cut).toBe(CUT.ALL);
        expect(cuts(plan.stations(main), CUT.LEFT)).toBeUndefined();
    });
});

describe('a level crossing', () => {
    const rail = () => way([[0, -100], [0, 0], [0, 100]], { kind: RAIL, code: RAIL_CODE, h: 1.9 });

    it('of a road with no junction changes nothing', () => {
        const road = through({ sideWays: [0, SIDE_BACK | SIDE_ON, 0] }), plan = planned([road, rail()]);
        expect(plan.junctions.corners).toEqual([]);
        expect(plan.stations(road).some(({ cut }) => cut)).toBe(false);
    });

    it('with a side road keeps the road\'s through pair, as the bake marks it', () => {
        const arm = stem([100, 100]), plan = planned([through(), arm, rail()]);
        expect(plan.junctions.mouths.map(({ way: { line } }) => line)).toEqual([arm]);
    });
});

describe('edges that cross at two heights', () => {
    it('are two levels, as at a ramp: no corner joins them', () => {
        const arm = stem([0, 40], { z: [1000, 1012], grade: [0.3, 0.3] }), plan = planned([through(), arm]);
        expect(plan.junctions.corners).toEqual([]);
        expect(plan.junctions.mouths).toEqual([]);
    });
});

describe('a junction with no through pair', () => {
    it('has no faces', () => {
        const a = way([[0, 0], [100, 0]]), b = way([[0, 0], [90, 40]]), c = way([[0, 0], [60, 80]]), plan = planned([a, b, c]);
        expect(plan.junctions.corners).toEqual([]);
    });
});

describe('a junction across a tile border', () => {
    it('plans alike from what each tile holds: a way cut short past the window changes nothing', () => {
        const whole = [through(), stem([0, 100])], cutA = [way([[-100, 0], [0, 0], [24, 0]]), stem([0, 100])], cutB = [way([[-24, 0], [0, 0], [100, 0]]), stem([0, 24])];
        const [A, B, C] = [whole, cutA, cutB].map(planned);
        const key = (plan: typeof A) => plan.junctions.corners.map(({ r, p, q, ends, meet }) => [r, ends[0] - p.along, ends[1] - q.along, meet[0] - p.along, meet[1] - q.along].map(v => Math.abs(v).toFixed(2)));
        expect(key(B)).toEqual(key(A));
        expect(key(C)).toEqual(key(A));
    });
});

describe('two lines of a road that OSM splits', () => {
    it('share a bank where they meet, whatever each fit ends on, so their edges stand on one height', () => {
        const a = way([[-100, 0], [0, 0]], { bank: [0.06, 0.06] }), b = way([[0, 0], [100, 0]], { bank: [-0.02, -0.02] }), plan = planned([a, b]);
        expect(plan.stations(a).at(-1)!.bank).toBeCloseTo(0.02, 6);
        expect(plan.stations(b)[0].bank).toBeCloseTo(0.02, 6);
    });

    it('share it across a change of sense: a rise to the left of the one is a rise to the right of the other', () => {
        const a = way([[-100, 0], [0, 0]], { bank: [0.06, 0.06] }), b = way([[100, 0], [0, 0]], { bank: [0.02, 0.02] }), plan = planned([a, b]);
        expect(plan.stations(a).at(-1)!.bank).toBeCloseTo(0.02, 6);
        expect(plan.stations(b).at(-1)!.bank).toBeCloseTo(-0.02, 6);
    });
});

describe('a mitre', () => {
    it('turns two lines\' end stations to one section where OSM splits a road at a bend', () => {
        const a = way([[-100, 0], [0, 0]]), b = way([[0, 0], [70, 70]]), plan = planned([a, b]), s = plan.stations(a).at(-1)!, t = plan.stations(b)[0];
        expect(s.ax).toBeCloseTo(t.ax, 9);
        expect(s.ay).toBeCloseTo(t.ay, 9);
        // Half of 45° each way, stretched so that the carriageway keeps its width.
        expect(Math.hypot(s.ax, s.ay) * unit).toBeCloseTo(1 / Math.cos(Math.PI / 8), 2);
    });
});

/** Whether a triangle of an index list holds a point, in metres east and north of O. */
function covers(b: PieceBuilder, list: number[], e: number, n: number): boolean {
    const piece = b.piece(0), ints = new Int16Array(piece.vertices), at = (i: number) => [(O.x + ints[8 * i] / 3 - O.x) * unit, (O.y - O.y - ints[8 * i + 1] / 3) * unit];
    for (let t = 0; t < list.length; t += 3) {
        const [p, q, r] = [list[t], list[t + 1], list[t + 2]].map(at), side = (u: number[], v: number[]) => (e - v[0]) * (u[1] - v[1]) - (u[0] - v[0]) * (n - v[1]), d = [side(p, q), side(q, r), side(r, p)];
        if (d.every(v => v <= 0) || d.every(v => v >= 0)) return true;
    }
    return false;
}

describe('the surface of a junction', () => {
    it('has no hole, in the near range and in the far range', () => {
        const main = through(), arm = stem([0, 100]), plan = planned([main, arm]), b = new PieceBuilder(O.x, O.y, 2 ** 13);
        for (const { far, stations, ...run } of plan.runs) {
            for (let j = 0; j + 1 < stations.length; j++) sweep(b, b.near, { ...run, stations }, j, j + 1, false);
            for (let k = 0; k + 1 < far.length; k++) sweep(b, b.far, { ...run, stations }, far[k], far[k + 1], true);
        }
        for (const corner of plan.junctions.corners) cornerFaces(b, corner);
        for (const mouth of plan.junctions.mouths) mouthFaces(b, mouth);
        // Metres from the road's middle to the arcs, which turn about (±8, 8), and up the arm.
        for (let e = -14.5; e <= 14.5; e += 1) {
            for (let n = 0.5; n <= 14.5; n += 1) {
                const onArc = Math.abs(e) >= 3 && n >= 3 && Math.abs(e) <= 8 && n <= 8 && Math.hypot(Math.abs(e) - 8, n - 8) >= 5.2;
                if (n > 2.9 && Math.abs(e) > 2.9 && !onArc) continue;
                expect(covers(b, b.near, e, n), `near ${e}, ${n}`).toBe(true);
                expect(covers(b, b.far, e, n), `far ${e}, ${n}`).toBe(true);
            }
        }
    });
});
