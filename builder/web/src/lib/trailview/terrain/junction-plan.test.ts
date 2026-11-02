import { describe, expect, it } from 'vitest';
import { SIDE_ON } from '../stream/features';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, SHOULDER_M, walkOf, type Line } from './bins';
import { alongOf, junctionPlan, RETURN_GRADE } from './junction-plan';
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
    it('stops where its middle crosses the road\'s outer edge, past the shoulder, on the road\'s plane, and is inside up to there', () => {
        const guest = path(), { guests: [plan] } = junctionPlan([road(), guest]), stop = 3 + SHOULDER_M.road!;
        expect(plan).toMatchObject({ host: expect.objectContaining({ kind: ROAD }), seam: stop, walk: false, mouth: false });
        expect(plan.stop).toBeCloseTo(stop, 3);
        expect(plan.z).toBeCloseTo(1000, 3);
        expect(rows(guest).filter(({ inside }) => inside).map(({ metres }) => metres)).toEqual([0]);
        expect(rows(guest)[1]).toMatchObject({ metres: expect.closeTo(stop, 3), inside: false });
    });

    it('stops on a sidewalk at its outer edge, a kerb over the plane, and at the shoulder on a side with none', () => {
        const { guests: [plan] } = junctionPlan([road({ sidewalks: LEFT | RIGHT }), path()]);
        expect(plan.stop).toBeCloseTo(3 + walkOf(road()), 3);
        expect(plan.z).toBeCloseTo(1000 + KERB_RISE_M, 3);
        expect(junctionPlan([road({ sidewalks: RIGHT }), path()]).guests[0]).toMatchObject({ walk: false, seam: 3 + SHOULDER_M.road! });
    });

    it('stops at an angle where its middle crosses the outer edge, on the road\'s banked plane there, held level past the carriageway', () => {
        const { guests: [plan] } = junctionPlan([road({ slope: 0.12, bank: 0.04 }), path({ angle: 50 })]), stop = (3 + SHOULDER_M.road!) / Math.cos((50 * Math.PI) / 180);
        expect(plan.stop).toBeCloseTo(stop, 2);
        expect(plan.z).toBeCloseTo(1000 + 0.12 * stop * Math.sin((50 * Math.PI) / 180) + 0.04 * 3, 2);
    });

    it('returns to its own heights past its stop, over 2 m at least and at 0.15 at most more than its own grade', () => {
        const guest = path({ grade: 0.1 }), { guests: [plan] } = junctionPlan([road(), guest]), own = (metres: number) => 1000 + 0.1 * metres;
        expect(plan.z).toBeCloseTo(1000, 3);
        const past = rows(guest).filter(({ metres }) => metres >= plan.stop - 1e-6);
        for (let k = 0; k + 1 < past.length; k++) expect((past[k + 1].z - past[k].z) / (past[k + 1].metres - past[k].metres) - 0.1).toBeLessThan(RETURN_GRADE * 1.5 + 1e-6);
        expect(past.filter(({ metres }) => metres > plan.stop + 2 + 0.34 / RETURN_GRADE).every(({ metres, z }) => Math.abs(z - own(metres)) < 1e-3)).toBe(true);
    });

    it('keeps to its half of the way to the next shared vertex, and moves less where that leaves no room', () => {
        const guest = path({ length: 14, grade: 0.6 }), other = way([[0, 14], [30, 14]], { kind: PATH, h: 0.6, code: 6, profile: profile('path'), z: [1008.4, 1008.4] });
        const { guests: [plan] } = junctionPlan([road(), guest, other]);
        // 2 m off its own height, with 7 - 3.4 m to the half: only 0.15 × that moves.
        expect(plan.z).toBeCloseTo(1000 + 0.6 * 3.4 - RETURN_GRADE * (7 - 3.4), 2);
        expect(guest.z[guest.z.length - 1]).toBe(1008.4);
    });

    it('as a driveway that crosses a sidewalk stops at its outer edge, and else keeps its mouth: only its height and its return', () => {
        const drive = () => way([[0, 0], [0, 60]], { sideWays: [SIDE_ON, 0], profile: profile('service'), h: 2, z: [1000, 1006], grade: [0.1, 0.1] });
        const crossing = drive(), { guests: [walked] } = junctionPlan([road({ sidewalks: LEFT }), crossing]);
        expect(walked).toMatchObject({ walk: true, mouth: false });
        expect(crossing.inside?.[0]).toBe(true);
        const mouth = drive(), { guests: [plain] } = junctionPlan([road(), mouth]);
        expect(plain).toMatchObject({ walk: false, mouth: true });
        expect(plain.z).toBeCloseTo(1000, 3);
        expect(mouth.inside).toBeUndefined();
    });

    it('as steps keeps their own heights', () => {
        const steps = path({ name: 'steps', grade: 0.5 }), { guests: [plan] } = junctionPlan([road(), steps]);
        expect(plan.z).toBeCloseTo(1000 + 0.5 * plan.stop, 3);
        expect(steps.inside?.[0]).toBe(true);
    });

    it('that ends inside the road, as a mapped crossing does, is inside to its end; a path that runs on from a road\'s end is no guest', () => {
        const crossing = path({ length: 3 }), { guests: [plan] } = junctionPlan([road(), crossing]);
        expect(plan.stop).toBeCloseTo(3, 1);
        expect(crossing.inside).toEqual([true]);
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
