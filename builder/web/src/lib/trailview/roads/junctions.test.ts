import { describe, expect, it } from 'vitest';
import { SIDE_BACK, SIDE_ON } from '../stream/features';
import { FILL_M, GUEST_GRADE, junctionPlan } from '../terrain/junction-plan';
import { junctionPlanes, KERB_RISE_M, metresPerUnit, PATH, PROFILES, RAIL, RAIL_CODE, RIVER, ROAD, SHOULDER_M, walkOf, type Line, type Shapes } from '../terrain/bins';
import { LEFT, RIGHT } from '../terrain/town';
import { cornerFaces, mouthFaces, seamFaces } from './junction-faces';
import { planOf } from './mesh';
import { heldAt } from './junctions';
import { PieceBuilder, ROAD_VERTEX } from './piece';
import { CF, LOOK, MAT } from './section';
import { CUT, stationIndex, stationOn, type Station } from './stations';
import { cap, plainEnd, sweep } from './sweep';

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
function planned(lines: Line[], areas: Shapes['areas'] = []) {
    const junctions = junctionPlan(lines);
    junctionPlanes(lines, junctions.guests);
    const plan = planOf({ lines, lakes: [], seas: [], areas, buildings: [], barriers: [], junctions }), of = (line: Line) => plan.runs.find(run => run.line === line)!;
    return { ...plan, of, stations: (line: Line) => of(line).stations };
}

/** The vertices of a piece, as a test reads them. */
function decoded(b: PieceBuilder) {
    const { vertices } = b.piece(0), words = new Uint32Array(vertices), floats = new Float32Array(vertices), ints = new Int16Array(vertices);
    return Array.from({ length: vertices.byteLength / ROAD_VERTEX }, (_, i) => ({
        e: (ints[8 * i] * 4 + ((words[4 * i + 2] >>> 28) & 3)) / 12 * unit, n: -(ints[8 * i + 1] * 4 + (words[4 * i + 2] >>> 30)) / 12 * unit, z: floats[4 * i + 1], mat: words[4 * i + 2] & 127, code: (words[4 * i + 2] >> 7) & 15, flags: (words[4 * i + 2] >> 11) & 31, across: ((words[4 * i + 3] << 20) >> 20) * 16 / 2047,
    }));
}

/** The vertices of a plan's faces at the junction. */
function faces(plan: ReturnType<typeof planned>) {
    const b = new PieceBuilder(O.x, O.y);
    for (const corner of plan.junctions.corners) cornerFaces(b, corner);
    for (const mouth of plan.junctions.mouths) mouthFaces(b, mouth);
    return { b, vertices: decoded(b) };
}

const through = (extra: Extra = {}) => way([[-100, 0], [0, 0], [100, 0]], extra);
const stem = (end: number[], extra: Extra = {}) => way([[0, 0], end], { sideWays: [SIDE_ON, 0], profile: profile('residential'), ...extra });

describe('a T', () => {
    const main = through(), side_ = stem([0, 100]), plan = planned([main, side_]);

    it('has two corners of a paved road\'s radius and a mouth', () => {
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
        const b = new PieceBuilder(O.x, O.y), run = plan.of(main), j = run.stations.findIndex(s => s.cut);
        sweep(b, b.near, run, j, j + 1, false);
        const flags = new Uint32Array(b.piece(0).vertices), first = (t: number) => (flags[4 * b.near[t] + 2] >> 11) & 31;
        expect(first(0) & CF.NO_LEFT_LINE).toBe(CF.NO_LEFT_LINE);
        expect(first(0) & CF.NO_RIGHT_LINE).toBe(0);
        const before = new PieceBuilder(O.x, O.y);
        sweep(before, before.near, run, 0, 1, false);
        expect((new Uint32Array(before.piece(0).vertices)[4 * before.near[0] + 2] >> 11) & 31).toBe(0);
    });

    it('fills the corners and the mouth in the through road\'s surface, also where the side road has another', () => {
        const { vertices } = faces(planned([through({ code: 1 }), stem([0, 100])])), fillet = vertices.filter(({ mat }) => mat === LOOK.ASPHALT);
        expect(fillet.length).toBeGreaterThan(0);
        expect(new Set(vertices.map(({ code }) => code))).toEqual(new Set([1]));
    });

    it('fills the mouth of a track of soil in its own look, its ruts run on to the road\'s edge, and turns no fillet', () => {
        const track = stem([0, 100], { profile: profile('track'), code: 4, h: 1.5 }), { vertices } = faces(planned([main, track]));
        expect(vertices.length).toBeGreaterThan(0);
        expect(vertices.every(({ mat }) => mat === LOOK.DIRT)).toBe(true);
        // Square to the road, a point of the mouth lies as far across the track as east of its line.
        for (const { e, across } of vertices) expect(across).toBeCloseTo(-e, 1);
    });

    it('holds a ring\'s metres across positive on both sides: a strip between signed ends would cross the middle', () => {
        expect(faces(plan).vertices.filter(({ flags }) => flags & (CF.SAMPLE | CF.MESH)).every(({ across }) => across > 0)).toBe(true);
        expect(faces(plan).vertices.every(({ across }) => across >= 0)).toBe(true);
    });

    it('rounds its arc tighter until another way\'s carriageway, 1 m past the through road\'s edge, passes clear of it', () => {
        const across = way([[-30, 7], [30, 7]]), plan = planned([through(), stem([0, 100]), across]);
        for (const { r } of plan.junctions.corners) expect(r > 0 && r < 1).toBe(true);
    });

    it('keeps its far range on the plan: each cut station, and the faces', () => {
        const { far, stations } = plan.of(main);
        stations.forEach((s, j) => s.cut && expect(far).toContain(j));
        const { b } = faces(plan);
        expect(b.far.length).toBeGreaterThan(0);
        expect(b.far.length).toBeLessThan(b.near.length);
    });
});

describe('a sharp corner\'s join', () => {
    // A side 3 m out with a 2 m sidewalk; the other way's edge lies g0 + g1·d from its point d metres out.
    it('ends a side whose edge lies in the other way\'s carriageway at its edge', () => {
        expect(heldAt(3, 2, 2, -5, 0.5)).toBe(3);
    });

    it('meets the other way\'s sidewalk on the bisector of an acute corner, and keeps what lies past the other\'s core', () => {
        // 1 m from the other edge at its own: the points as far from both edges lie 2/3 m out.
        expect(heldAt(3, 2, 2, 2.5, -0.5)).toBeCloseTo(3 + 2 / 3, 6);
        // 5 m from it, past the other's 2 m sidewalk: it keeps its points up to where that sidewalk starts.
        expect(heldAt(3, 2, 2, 6.5, -0.5)).toBeCloseTo(9, 6);
    });

    it('keeps the whole of a wider core where the other\'s would cut a hole in it, and gives way where its own is the narrower', () => {
        expect(heldAt(3, 2, 0.4, -1.5, 0.5)).toBe(Infinity);
        expect(heldAt(3, 0.4, 2, -1.5, 0.5)).toBeCloseTo(3, 6);
    });
});

describe('a corner\'s radius', () => {
    const widest = (plan: ReturnType<typeof planned>) => Math.max(...plan.junctions.corners.map(({ r }) => r));

    it('is a paved road\'s on each side of a side road, whatever the turn and the town', () => {
        // 152 degrees between the arm and the road's east way, and 28 between it and the west way.
        expect(widest(planned([through(), stem([-88, 47])]))).toBe(5);
        expect(widest(planned([through(), stem([-88, 47], { sidewalks: [LEFT] })]))).toBe(5);
    });
});

describe('a side arm that is one line with a through way', () => {
    it('turns no corner on that line\'s inner edge: the edge runs on, and the mouth ends on it', () => {
        const line = way([[0, 60], [0, 0], [60, 0]], { sideWays: [0, SIDE_BACK, 0], profile: profile('residential') }), plan = planned([line, way([[-60, 0], [0, 0]], { profile: profile('residential') })]);
        expect(plan.junctions.mouths).toHaveLength(1);
        const inner = plan.junctions.corners.find(({ oneLine }) => oneLine)!;
        expect(inner.r).toBe(0);
        // The line is its own arm and through way: the side toward the inner bend stays whole.
        const side = inner.p.line === line ? inner.sp : inner.sq;
        expect(cuts(plan.stations(line), side * inner.p.step > 0 ? CUT.LEFT : CUT.RIGHT)).toBeDefined();
        const { b, vertices } = faces(plan);
        expect(b.near.length).toBeGreaterThan(0);
        expect(vertices.length).toBeLessThan(200);
    });
});

describe('a corner near the next junction', () => {
    it('stays within its share of the way there', () => {
        const arm = way([[0, 0], [0, 12], [0, 100]], { sideWays: [SIDE_ON, 0, 0], profile: profile('residential') }), plan = planned([through(), arm, way([[0, 12], [80, 12]])]);
        const corners = plan.junctions.corners.filter(({ meeting }) => meeting.x === O.x);
        expect(corners.length).toBeGreaterThan(0);
        for (const { q, p, ends } of corners) {
            const end = p.line === arm ? ends[0] : ends[1];
            expect(end - (p.line === arm ? p : q).along).toBeLessThanOrEqual(0.45 * 12 + 0.01);
        }
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

describe('a mouth', () => {
    /**
     * A side arm's mouth off the through road, by the end of its 100 m line: its near faces in metres about the
     * junction, and the arm's carriageway from its junction to its stop that lies off the through road's.
     */
    const mouthOf = (end: number[], extra: Extra = {}) => {
        const main = through(), arm = stem(end, extra), [mouth] = planned([main, arm]).junctions.mouths, b = new PieceBuilder(O.x, O.y);
        mouthFaces(b, mouth);
        const v = decoded(b), l = Math.hypot(end[0], end[1]), [ux, uy] = [end[0] / l, end[1] / l];
        const faces = Array.from({ length: b.near.length / 3 }, (_, f) => [0, 1, 2].map(k => v[b.near[3 * f + k]]));
        const inside = (e: number, n: number, margin: number) => {
            const along = e * ux + n * uy, off = Math.abs(n * ux - e * uy);
            return n >= main.h - margin && along >= -margin && along <= mouth.far + margin && off <= arm.h + margin;
        };
        return { faces, inside };
    };
    const MARGIN_M = 0.02, STEP_M = 0.05;

    for (const [name, end, extra] of [['square to the road', [0, 100], {}], ['at 30°', [86.6, 50], {}], ['as a track of soil at 20°, its mouth 13 m long', [93.97, 34.2], { profile: profile('track'), code: 4, h: 1.5 }]] as const) {
        it(`${name}: covers the arm's carriageway off the through road once, in faces that face up, and nothing past it`, () => {
            const { faces, inside } = mouthOf([...end], extra), within = ([a, b, c]: { e: number; n: number }[], e: number, n: number) => {
                const side = (p: { e: number; n: number }, q: { e: number; n: number }) => (q.e - p.e) * (n - p.n) - (q.n - p.n) * (e - p.e);
                return side(a, b) >= 0 && side(b, c) >= 0 && side(c, a) >= 0;
            };
            for (const face of faces) {
                const [a, b, c] = face, up = (b.e - a.e) * (c.n - a.n) - (b.n - a.n) * (c.e - a.e);
                expect(up).toBeGreaterThan(0);
                expect(inside((a.e + b.e + c.e) / 3, (a.n + b.n + c.n) / 3, MARGIN_M)).toBe(true);
                // Flat ground gives a flat mouth: a fold or a flap would stand up from it.
                expect(Math.max(...face.map(({ z }) => z)) - Math.min(...face.map(({ z }) => z))).toBeLessThan(0.1);
            }
            let samples = 0, covered = 0;
            for (let e = -30; e <= 30; e += STEP_M) for (let n = 0; n <= 30; n += STEP_M) {
                if (!inside(e, n, -MARGIN_M)) continue;
                samples++;
                const holding = faces.filter(face => within(face, e, n)).length;
                expect(holding).toBeLessThanOrEqual(1);
                covered += holding;
            }
            expect(covered).toBe(samples);
        });
    }
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
        const main = through({ bank: [bank, bank, bank] }), arm = stem([0, 100], { bank: [0.04, 0.04] }), plan = planned([main, arm]);
        const stations = plan.stations(arm), at = (metres: number) => stations.reduce((a, s) => (Math.abs(s.along - metres) < Math.abs(a.along - metres) ? s : a));
        return { main, arm, plan, stations, at };
    };
    const edges = (s: Station) => [s.z + s.bank * s.h, s.z - s.bank * s.h];

    it('stops on the through road\'s banked plane, each edge on it whatever its own bank, and returns to its own heights', () => {
        const { at, plan } = banked(0.06), T = stationOn(plan.stations(plan.junctions.mouths[0].way.line), plan.junctions.mouths[0].far);
        for (const z of edges(T)) expect(z).toBeCloseTo(1000 + 0.06 * 3, 2);
        expect(at(40).z).toBeCloseTo(1000, 3);
    });

    it('leaves the through road where it is: no height of the host moves', () => {
        const { main, plan } = banked(-0.06);
        for (const s of plan.stations(main)) {
            expect(s.z).toBeCloseTo(1000, 6);
            expect(s.bank).toBeCloseTo(-0.06, 6);
        }
    });

    it('takes the plane of its mouth: on a grade its edges stand on the through road\'s edge on each side', () => {
        const main = through({ z: [990, 1000, 1010], grade: [0.1, 0.1, 0.1] }), arm = stem([0, 100]), plan = planned([main, arm]), T = stationOn(plan.stations(arm), plan.junctions.mouths[0].far);
        // The arm runs north: its left edge is the west one, down the grade.
        expect(edges(T)[0]).toBeCloseTo(1000 - 0.1 * 3, 2);
        expect(edges(T)[1]).toBeCloseTo(1000 + 0.1 * 3, 2);
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

    it('through a junction with a paved road leaves its faces to the road\'s surface', () => {
        const road = way([[-100, 0], [0, 0]]), plan = planned([way([[0, 0], [100, 0]], { profile: profile('track'), code: gravel }), road, stem([0, 100])]);
        expect(plan.junctions.corners).not.toHaveLength(0);
        for (const corner of plan.junctions.corners) expect(corner.road).toBe(road);
    });

    it('that joins at a hairpin\'s apex has a mouth on the road, which the bake keeps through', () => {
        const road = way(arc([0, 4], 4, -180, 0)), track = stem([0, -60], { profile: profile('track'), code: gravel }), plan = planned([road, track]);
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

    it('as a path stops FILL_M past the road\'s outer edge, past its shoulder, on the road\'s plane', () => {
        const main = through(), path = lane(), plan = planned([main, path]), s = plan.stations(path)[0], t = plan.stations(path).find(({ cut }) => !cut)!;
        expect(s.cut).toBe(CUT.ALL);
        // The half-width and the verge of a road code.
        expect(t.along).toBeCloseTo(3 + SHOULDER_M.road! + FILL_M, 2);
        expect(t.z).toBeCloseTo(1000, 3);
    });

    it('on a sidewalk stops past it, on its level', () => {
        const main = through({ sidewalks: [LEFT | RIGHT, LEFT | RIGHT], profile: profile('residential') }), path = lane(), plan = planned([main, path]), t = plan.stations(path).find(({ cut }) => !cut)!;
        expect(t.along).toBeCloseTo(3 + walkOf(main) + FILL_M, 2);
        expect(t.z).toBeCloseTo(1000 + KERB_RISE_M, 3);
    });

    it('fills the ground between its stop and the road\'s outer edge, where it leaves at an angle, in its own look', () => {
        const main = through(), path = lane([50, 50]), plan = planned([main, path]), [seam] = plan.junctions.seams, t = plan.stations(path).find(({ cut }) => !cut)!;
        // Its right edge leaves the outer edge, 3.4 m out, 3.4 √2 + 1 m along it, the last of its core; it stops FILL_M past that.
        expect(t.along).toBeCloseTo(3.4 * Math.SQRT2 + 1 + FILL_M, 2);
        const b = new PieceBuilder(O.x, O.y);
        seamFaces(b, seam);
        const vertices = decoded(b), used = [...new Set(b.near)].map(i => vertices[i]);
        expect(b.near.length).toBeGreaterThan(0);
        expect(used.filter(({ flags }) => !(flags & (CF.SAMPLE | CF.MESH))).every(({ mat }) => mat === LOOK.TRAIL || mat === MAT.GROUND)).toBe(true);
        // Each point holds its metres across the path, so the fill's look runs on from the path's own, its edges with it.
        for (const { e, n, across, flags } of used) if (!(flags & (CF.SAMPLE | CF.MESH))) expect(across).toBeCloseTo((n - e) / Math.SQRT2, 1);
        // From the stop's section back to where the path's left edge crosses the outer edge, 3.4 √2 - 1 m along it.
        expect(Math.min(...used.map(({ e, n }) => (e + n) / Math.SQRT2))).toBeCloseTo(3.4 * Math.SQRT2 - 1, 1);
        expect(Math.min(...used.map(({ n }) => n))).toBeCloseTo(3.4, 1);
    });

    it('as a driveway on a kerbed road ends at its outer edge too, with no corner', () => {
        const main = through({ sidewalks: [LEFT, LEFT], profile: profile('residential') }), drive = stem([0, 60], { profile: profile('service'), driveway: true }), plan = planned([main, drive]);
        expect(plan.junctions.corners).toEqual([]);
        expect(plan.stations(drive)[0].cut).toBe(CUT.ALL);
        expect(cuts(plan.stations(main), CUT.LEFT)).toBeUndefined();
    });
});

describe('a road that a path runs on from', () => {
    const footway = (extra: Partial<Line> = {}) => way([[0, 0], [30, 0]], { kind: PATH, h: 0.6, profile: profile('path'), code: 6, ...extra });
    const road = () => way([[-50, 0], [0, 0]], { profile: profile('residential'), sidewalks: [LEFT | RIGHT] });

    it('ends plainly, with no kerb or sidewalk across the path', () => {
        const plan = planned([road(), footway()]), run = plan.runs[0], end = run.caps.find(({ plain }) => plain)!;
        expect(end).toEqual({ j: run.stations.length - 1, dir: 1, plain: true });
        const b = new PieceBuilder(O.x, O.y);
        plainEnd(b, b.near, run, end.j, end.dir, false);
        const ahead = decoded(b).filter(({ e }) => e > 0.1);
        expect(ahead.length).toBeGreaterThan(0);
        expect(ahead.every(({ mat }) => mat === MAT.GROUND)).toBe(true);
    });

    it('keeps its round end where no way runs on', () => {
        expect(planned([road()]).runs[0].caps.some(({ plain }) => plain)).toBe(false);
    });

    it('lets a path that leaves at a side stop at the sidewalk, and one that runs straight on start at the road', () => {
        const back = way([[0, 0], [-20, 20]], { kind: PATH, h: 0.6, profile: profile('path'), code: 6 });
        expect(planned([road(), back]).junctions.seams.map(({ walk }) => walk)).toEqual([true]);
        expect(planned([road(), footway()]).junctions.seams).toEqual([]);
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

describe('a side arm that leaves the through road steeply', () => {
    it('stops on the through road\'s plane and climbs back to its own heights past there, on one surface with no step', () => {
        const arm = stem([0, 40], { z: [1000, 1012], grade: [0.3, 0.3] }), plan = planned([through(), arm]), stations = plan.stations(arm), j = stationIndex(stations, plan.junctions.mouths[0].far);
        expect(stations[j].z).toBeCloseTo(1000, 2);
        for (let k = j; k + 1 < stations.length; k++) expect((stations[k + 1].z - stations[k].z) / (stations[k + 1].along - stations[k].along)).toBeLessThan(0.3 + 1.5 * GUEST_GRADE);
        expect(stations.at(-1)!.z).toBeCloseTo(1012, 3);
    });
});

describe('a side arm far under the through road', () => {
    it('climbs to its plane over the metres past its mouth, with no step', () => {
        const arm = stem([0, 100]), plan = planned([through({ z: [1002, 1002, 1002] }), arm]), stations = plan.stations(arm), j = stationIndex(stations, plan.junctions.mouths[0].far);
        expect(stations[j].z).toBeCloseTo(1002, 1);
        for (let k = j; k < j + 8; k++) expect(Math.abs(stations[k + 1].z - stations[k].z) / (stations[k + 1].along - stations[k].along)).toBeLessThan(0.35);
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
        const [A, B, C] = [whole, cutA, cutB].map(lines => planned(lines));
        const key = (plan: typeof A) => plan.junctions.corners.map(({ r, p, q, ends, meet }) => [r, ends[0] - p.along, ends[1] - q.along, meet[0] - p.along, meet[1] - q.along].map(v => Math.abs(v).toFixed(2)));
        expect(key(B)).toEqual(key(A));
        expect(key(C)).toEqual(key(A));
    });
});

describe('two lines of a road that OSM splits', () => {
    const edge = (s: Station, side: number) => s.z + s.bank * side * s.h;

    it('meet with no crack where a side arm that leaves a through plane is split', () => {
        const main = through({ bank: [0.06, 0.06, 0.06] }), a = way([[0, 0], [0, 7]], { sideWays: [SIDE_ON, 0], profile: profile('residential') }), b = way([[0, 7], [0, 100]], { profile: profile('residential') }), plan = planned([main, a, b]);
        const [s, t] = [plan.stations(a).at(-1)!, plan.stations(b)[0]];
        for (const side of [1, -1]) expect(edge(s, side)).toBeCloseTo(edge(t, side), 3);
    });

    it('fade as one where the lines run head to head: a left is a right across the joint', () => {
        const main = through({ bank: [0.06, 0.06, 0.06] }), a = way([[0, 0], [0, 7]], { sideWays: [SIDE_ON, 0], profile: profile('residential') }), b = way([[0, 100], [0, 7]], { profile: profile('residential') }), plan = planned([main, a, b]);
        const [s, t] = [plan.stations(a).at(-1)!, plan.stations(b).at(-1)!];
        for (const side of [1, -1]) expect(edge(s, side)).toBeCloseTo(edge(t, -side), 3);
    });

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

describe('a closed line', () => {
    /** A circle of radius r metres that ends on its first point, with a side road at that point. */
    const ring = (r: number, extra: Extra = {}) => {
        const line = way(arc([0, 0], r, 0, 360, 15), extra);
        line.xy.splice(-2, 2, line.xy[0], line.xy[1]);
        return [line, way([[r, 0], [r + 60, 0]], { sideWays: [SIDE_ON, 0], profile: profile('residential') })];
    };

    it('joins its end to its other end, not to itself, so its edges hold across the joint', () => {
        const lines = ring(30, { bank: new Array(25).fill(0.06) }), plan = planned(lines), stations = plan.stations(lines[0]), [s, t] = [stations[0], stations.at(-1)!];
        expect(s.bank).toBeGreaterThan(0.01);
        for (const side of [1, -1]) expect(s.z + s.bank * side * s.h).toBeCloseTo(t.z + t.bank * side * t.h, 3);
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
    const vertices = decoded(b), at = (i: number) => [vertices[i].e, vertices[i].n];
    for (let t = 0; t < list.length; t += 3) {
        const [p, q, r] = [list[t], list[t + 1], list[t + 2]].map(at), side = (u: number[], v: number[]) => (e - v[0]) * (u[1] - v[1]) - (u[0] - v[0]) * (n - v[1]), d = [side(p, q), side(q, r), side(r, p)];
        if (d.every(v => v <= 0) || d.every(v => v >= 0)) return true;
    }
    return false;
}

describe('the surface of a junction', () => {
    it('has no hole, in the near range and in the far range', () => {
        const main = through(), arm = stem([0, 100]), plan = planned([main, arm]), b = new PieceBuilder(O.x, O.y);
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

describe('a way at the water', () => {
    /** The vertices of a plan's corridors, its dead ends and its corners. */
    function drawn(plan: ReturnType<typeof planned>) {
        const b = new PieceBuilder(O.x, O.y);
        for (const run of plan.runs) {
            for (let j = 0; j + 1 < run.stations.length; j++) sweep(b, b.near, run, j, j + 1, false);
            for (const { j, dir } of run.caps) cap(b, b.near, run, j, dir, false);
        }
        for (const corner of plan.junctions.corners) cornerFaces(b, corner);
        return decoded(b);
    }
    const river = (e0: number, n0: number, e1: number, n1: number, h: number) => way([[e0, n0], [e1, n1]], { kind: RIVER, h });
    const road = () => way([[-50, 0], [50, 0]]);

    it('stops its side at a river beside it, and its other side keeps its band', () => {
        const ns = drawn(planned([road(), river(-200, 7, 200, 7, 2)])).map(({ n }) => n);
        expect(Math.max(...ns)).toBeCloseTo(5, 1);
        expect(Math.min(...ns)).toBeCloseTo(-6.6, 1);
    });

    it('keeps its carriageway and a narrow verge where the water reaches them', () => {
        expect(Math.max(...drawn(planned([road(), river(-200, 4, 200, 4, 2)])).map(({ n }) => n))).toBeCloseTo(3.4, 1);
    });

    it('stops the rings of a dead end at the water', () => {
        expect(Math.max(...drawn(planned([road(), river(55, -200, 55, 200, 0.5)])).map(({ e }) => e))).toBeCloseTo(54.5, 1);
    });

    it('stops the rows of a corner at a lake in its wedge', () => {
        const [low, high] = [5.5, 30], ring = [[low, low], [high, low], [high, high], [low, high]].flatMap(([e, n]) => [O.x + e / unit, O.y - n / unit]);
        const lines = () => [through(), stem([0, 100])], beyond = (vertices: { e: number; n: number }[]) => Math.max(...vertices.filter(({ e, n }) => e < high && n < high).map(({ e, n }) => Math.min(e, n)));
        expect(beyond(drawn(planned(lines())))).toBeGreaterThan(low + 1);
        expect(beyond(drawn(planned(lines(), [{ rings: [ring], outer: [true] }])))).toBeLessThan(low + 0.05);
    });
});
