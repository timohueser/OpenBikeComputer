import { describe, expect, it } from 'vitest';
import { SIDE_ON } from '../stream/features';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, walkOf, type Line } from './bins';
import { alongOf, FILL_M, GUEST_GRADE, GUEST_INSIDE, GUEST_MOUTH, junctionPlan, RETURN_GRADE, WALK_END_M } from './junction-plan';
import { LEFT, RIGHT } from './town';

const O = { x: 2 ** 28, y: 2 ** 29 }, unit = metresPerUnit(O.y), profile = (name: string) => Object.keys(PROFILES).indexOf(name);

/** A line through points in metres east and north of O, straight between them, with tangents along its chords. */
function way(points: number[][], { grade, bank, ...extra }: Partial<Line> & { grade?: number[]; bank?: number[] } = {}): Line {
    const count = points.length, zero = new Array(count).fill(0), tangent = points.map((_, v) => {
        const [a, b] = [points[Math.max(0, v - 1)], points[Math.min(count - 1, v + 1)]];
        return Math.atan2(a[1] - b[1], b[0] - a[0]);
    });
    return {
        kind: ROAD, xy: points.flatMap(([e, n]) => [Math.round(O.x + e / unit), Math.round(O.y - n / unit)]), z: new Array(count).fill(1000), h: 3, code: 2, ford: false, profile: profile('residential'),
        anchors: new Array(count).fill(true), plan: { tangent, grade: grade ?? zero, bank: bank ?? zero, widen: zero }, ...extra,
    };
}

/** A road east through O, `slope` per metre east, banked `bank` to its left, with sidewalks or none. */
const road = ({ slope = 0, bank = 0, sidewalks = 0 } = {}) => way([[-50, 0], [0, 0], [50, 0]], { z: [1000 - 50 * slope, 1000, 1000 + 50 * slope], grade: [slope, slope, slope], bank: [bank, bank, bank], sidewalks: sidewalks ? [sidewalks, sidewalks] : undefined });
/** A path from O at `angle` degrees east of north, `length` metres, climbing `grade` per metre. */
const path = ({ angle = 0, length = 40, grade = 0, name = 'path' } = {}) => {
    const t = (angle * Math.PI) / 180;
    return way([[0, 0], [length * Math.sin(t), length * Math.cos(t)]], { kind: PATH, h: 0.6, code: 6, profile: profile(name), z: [1000, 1000 + length * grade], grade: [grade, grade] });
};
/** A line's height and its share of `inside`, per vertex, by metres from its first vertex. */
const rows = (line: Line) => alongOf(line).map((metres, v) => ({ metres, z: line.z[v], inside: !!line.inside?.[v] }));

describe('a guest of a road', () => {
    it('stops FILL_M past where its middle crosses the road\'s outer edge, past the shoulder, on the road\'s plane, and is inside up to there', () => {
        const guest = path(), { guests: [plan] } = junctionPlan([road(), guest]), stop = 3 + SHOULDER_M.road! + FILL_M;
        expect(plan).toMatchObject({ kind: 'path', host: expect.objectContaining({ kind: ROAD }), seam: 3 + SHOULDER_M.road!, walk: false });
        expect(plan.stop).toBeCloseTo(stop, 3);
        expect(plan.z).toBeCloseTo(1000, 3);
        expect(rows(guest).filter(({ inside }) => inside).map(({ metres }) => metres)).toEqual([0]);
        expect(rows(guest)[1]).toMatchObject({ metres: expect.closeTo(stop, 3), inside: false });
    });

    it('stops on a sidewalk FILL_M past its outer edge, a kerb over the plane, and at the shoulder on a side with none', () => {
        const { guests: [plan] } = junctionPlan([road({ sidewalks: LEFT | RIGHT }), path()]);
        expect(plan.stop).toBeCloseTo(3 + walkOf(road()) + FILL_M, 3);
        expect(plan.z).toBeCloseTo(1000 + KERB_RISE_M, 3);
        expect(junctionPlan([road({ sidewalks: RIGHT }), path()]).guests[0]).toMatchObject({ walk: false, seam: 3 + SHOULDER_M.road! });
    });

    it('stops at an angle FILL_M past where its whole core has left the outer edge, on the road\'s banked plane under its middle, held level past the carriageway', () => {
        const t = (50 * Math.PI) / 180, { guests: [plan] } = junctionPlan([road({ slope: 0.12, bank: 0.04 }), path({ angle: 50 })]), stop = (3 + SHOULDER_M.road!) / Math.cos(t) + 0.6 * Math.tan(t) + FILL_M;
        expect(plan.stop).toBeCloseTo(stop, 2);
        expect(plan.z).toBeCloseTo(1000 + 0.12 * stop * Math.sin(t) + 0.04 * 3, 2);
    });

    it('returns to its own heights past its stop, over 2 m at least and at 0.15 at most more than its own grade', () => {
        const guest = path({ grade: 0.1 }), { guests: [plan] } = junctionPlan([road(), guest]), own = (metres: number) => 1000 + 0.1 * metres;
        expect(plan.z).toBeCloseTo(1000, 3);
        const past = rows(guest).filter(({ metres }) => metres >= plan.stop - 1e-6);
        for (let k = 0; k + 1 < past.length; k++) expect((past[k + 1].z - past[k].z) / (past[k + 1].metres - past[k].metres) - 0.1).toBeLessThan(RETURN_GRADE * 1.5 + 1e-6);
        expect(past.filter(({ metres }) => metres > plan.stop + 2 + 0.34 / RETURN_GRADE).every(({ metres, z }) => Math.abs(z - own(metres)) < 1e-3)).toBe(true);
    });

    it('ends its return 2 m short of the next shared vertex, steeper where that leaves too little room, and moves less past that', () => {
        const run = (grade: number) => {
            const guest = path({ length: 14, grade }), other = way([[0, 14], [30, 14]], { kind: PATH, h: 0.6, code: 6, profile: profile('path'), z: [1000 + 14 * grade, 1000 + 14 * grade] });
            const { guests: [plan] } = junctionPlan([road(), guest, other]);
            expect(guest.z[guest.z.length - 1]).toBe(1000 + 14 * grade);
            return plan.z;
        };
        // 2 m off its own height, with 14 - 2 - 3.9 m of room: a guest's grade carries it.
        expect(run(0.5)).toBeCloseTo(1000, 2);
        // 10 m off: only GUEST_GRADE × the room moves.
        expect(run(3)).toBeCloseTo(1000 + 3 * 3.9 - GUEST_GRADE * (14 - 2 - 3.9), 1);
    });

    it('as a driveway (service=driveway) that crosses a sidewalk stops at its outer edge, a kerb over the plane', () => {
        const drive = way([[0, 0], [0, 60]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2, z: [1000, 1006], grade: [0.1, 0.1], driveway: true });
        const { guests: [walked], corners } = junctionPlan([road({ sidewalks: LEFT }), drive]);
        expect(walked).toMatchObject({ kind: 'driveway', walk: true, seam: 3 + walkOf(road({ sidewalks: LEFT })) });
        expect(walked.z).toBeCloseTo(1000 + KERB_RISE_M, 3);
        expect(drive.inside?.[0]).toBe(GUEST_INSIDE);
        expect(corners).toEqual([]);
    });

    it('as steps keeps their own heights', () => {
        const steps = path({ name: 'steps', grade: 0.5 }), { guests: [plan] } = junctionPlan([road(), steps]);
        expect(plan.z).toBeCloseTo(1000 + 0.5 * plan.stop, 3);
        expect(steps.inside?.[0]).toBe(GUEST_INSIDE);
    });

    it('that ends inside the road, as a mapped crossing does, is inside to its end; a path that runs on from a road\'s end is no guest', () => {
        const crossing = path({ length: 3 }), { guests: [plan] } = junctionPlan([road(), crossing]);
        expect(plan.stop).toBeCloseTo(3, 1);
        expect(crossing.inside).toEqual([GUEST_INSIDE]);
        const end = way([[-50, 0], [0, 0]]), on = way([[0, 0], [40, 0]], { kind: PATH, h: 0.6, code: 6, profile: profile('path') }), side = path({ angle: -45 });
        expect(junctionPlan([end, on]).guests).toEqual([]);
        expect(junctionPlan([way([[-50, 0], [0, 0]]), side]).guests).toHaveLength(1);
    });

    it('plans alike in two tiles that cut its lines elsewhere', () => {
        const plans = [[[-50, 0], [0, 0], [50, 0]], [[-12, 0], [0, 0], [50, 0]]].map(points => {
            const guest = path({ grade: 0.2 }), cut = way(points);
            junctionPlan([cut, guest]);
            return rows(guest).filter(({ metres }) => metres < 30);
        });
        expect(plans[1]).toEqual(plans[0]);
    });
});

describe('a side road', () => {
    /** A service road north from O, `slope` per metre, on a road with sidewalks or none. */
    const side = (extra: Partial<Line> & { grade?: number[] } = {}) => way([[0, 0], [0, 60]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2, ...extra });

    it('is every road out of a junction that is no driveway: it stops where both its edges have met the carriageway edge, between two corners', () => {
        const guest = side(), { guests: [plan], corners } = junctionPlan([road({ sidewalks: LEFT | RIGHT }), guest]);
        expect(plan).toMatchObject({ kind: 'side', seam: 3, walk: false });
        // Square to the road: both edges meet its carriageway edge 3 m out.
        expect(plan.stop).toBeCloseTo(3, 1);
        expect(guest.inside?.[0]).toBe(GUEST_MOUTH);
        expect(corners.map(({ r }) => r)).toEqual([5, 5]);
    });

    it('stands on the plane of its mouth where it stops, and returns to its own heights past there', () => {
        const guest = side({ z: [1000, 1012], grade: [0.2, 0.2] }), { guests: [plan] } = junctionPlan([road({ slope: 0.1, bank: 0.04 }), guest]);
        // On the road's banked plane at its edge, 3 m north of its middle: the bank rises to the road's left, north.
        expect(plan.z).toBeCloseTo(1000 + 0.04 * 3, 2);
        expect(plan.plane![0]).toBeCloseTo(0.1, 2);
        const far = rows(guest).filter(({ metres }) => metres > 3 + (0.6 - 0.12) / RETURN_GRADE + 1e-3);
        for (const { metres, z } of far) expect(z).toBeCloseTo(1000 + 0.2 * metres, 2);
    });

    it('takes its host\'s sidewalk round each corner to WALK_END_M past its arc, where the sidewalk ends', () => {
        const host = road({ sidewalks: LEFT | RIGHT }), guest = side(), { corners } = junctionPlan([host, guest]);
        expect(guest.walk).toBe(walkOf(host));
        const along = alongOf(guest), walked = guest.sidewalks!.map((bits, s) => ({ bits, end: along[s + 1] })).filter(({ bits }) => bits === (LEFT | RIGHT));
        const arc = Math.max(...corners.map(({ p, ends }) => (p.line === guest ? ends[0] : ends[1])));
        expect(walked.at(-1)!.end).toBeCloseTo(arc + WALK_END_M, 1);
        // A road without sidewalks gives none.
        const plain = side();
        junctionPlan([road(), plain]);
        expect(plain.sidewalks?.some(Boolean) ?? false).toBe(false);
    });

    it('rounds an acute corner tighter, so its arc ends no farther from where the edges meet than at a right angle', () => {
        // 40° between the road's east way and the side road.
        const t = (50 * Math.PI) / 180, guest = way([[0, 0], [60 * Math.sin(t), 60 * Math.cos(t)]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2 });
        const { corners } = junctionPlan([road(), guest]), acute = corners.find(({ r }) => r < 5)!;
        expect(acute.r).toBeGreaterThan(0);
        for (const k of [0, 1]) expect(Math.abs(acute.ends[k] - acute.meet[k])).toBeLessThanOrEqual(5 + 0.01);
    });

    it('as a way of soil leaves its corners sharp, but where a sidewalk turns round them', () => {
        const track = () => side({ profile: profile('track'), code: 4, h: 1.5 });
        expect(junctionPlan([road(), track()]).corners.map(({ r }) => r)).toEqual([0, 0]);
        for (const { r } of junctionPlan([road({ sidewalks: LEFT | RIGHT }), track()]).corners) expect(r).toBeGreaterThan(0);
    });

    it('leaving at a wide angle has corners whose arcs end ahead of the junction', () => {
        // 150° between the road's west way and the side road.
        const t = (60 * Math.PI) / 180, guest = way([[0, 0], [60 * Math.sin(t), 60 * Math.cos(t)]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2 });
        const { corners, guests } = junctionPlan([road(), guest]);
        expect(guests).toHaveLength(1);
        // Metres along a line at the junction's vertex, which the plan's cuts move in the list.
        const junction = (line: Line) => alongOf(line)[line.xy.findIndex((value, i) => i % 2 === 0 && value === Math.round(O.x) && line.xy[i + 1] === Math.round(O.y)) / 2];
        for (const { r, ends, p, q } of corners) {
            expect(r).toBeGreaterThan(0);
            expect((ends[0] - junction(p.line)) * p.step).toBeGreaterThanOrEqual(-1e-6);
            expect((ends[1] - junction(q.line)) * q.step).toBeGreaterThanOrEqual(-1e-6);
        }
    });
});

