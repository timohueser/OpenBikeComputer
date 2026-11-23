import { describe, expect, it } from 'vitest';
import { grownHull, hull, span, type Point } from './town-area';

const p = (x: number, y: number): Point => ({ x, y });
/** The distance from `point` to the segment from `a` to `b`. */
function toSegment(point: Point, a: Point, b: Point): number {
    const dx = b.x - a.x, dy = b.y - a.y, t = Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / (dx * dx + dy * dy || 1)));
    return Math.hypot(point.x - a.x - t * dx, point.y - a.y - t * dy);
}
/** The distance from `point` to the edge of the polygon `corners`. */
const toEdge = (point: Point, corners: Point[]) => Math.min(...corners.map((corner, i) => toSegment(point, corner, corners[(i + 1) % corners.length])));

describe('town area', () => {
    it('takes the outer points of scattered points as the corners of the hull', () => {
        const square = [p(0, 0), p(10, 0), p(10, 10), p(0, 10)];
        const corners = hull([p(5, 5), ...square, p(5, 0), p(2, 7), p(10, 10)]);
        expect(corners).toHaveLength(4);
        expect(corners).toEqual(expect.arrayContaining(square));
    });

    it('grows the hull by the radius, with a round corner at each point of the hull', () => {
        const corners = [p(0, 0), p(40, 0), p(40, 30), p(0, 30)];
        const ring = grownHull([...corners, p(20, 15)], 15);
        for (const point of ring) expect(toEdge(point, corners)).toBeCloseTo(15, 6);
        // Each corner turns a quarter in steps too short to show.
        expect(ring.length).toBeGreaterThanOrEqual(4 * 8);
        expect([Math.min(...ring.map(q => q.x)), Math.max(...ring.map(q => q.x)), Math.min(...ring.map(q => q.y)), Math.max(...ring.map(q => q.y))])
            .toEqual([-15, 55, -15, 45].map(value => expect.closeTo(value, 6)));
    });

    it('makes a capsule of two points and a circle of one, also when places share a point', () => {
        const a = p(10, 10), b = p(50, 40);
        const capsule = grownHull([a, b, a], 15);
        for (const point of capsule) expect(toSegment(point, a, b)).toBeCloseTo(15, 6);
        // Both ends are round: the outline reaches the radius beyond each point.
        const along = (point: Point) => ((point.x - a.x) * 40 + (point.y - a.y) * 30) / 50;
        expect([Math.min(...capsule.map(along)), Math.max(...capsule.map(along))]).toEqual([expect.closeTo(-15, 1), expect.closeTo(65, 1)]);
        // At the height of its middle, the slanted capsule is narrower than its box.
        expect(span(capsule, 25)).toEqual([expect.closeTo(5, 6), expect.closeTo(55, 6)]);

        const circle = grownHull([a, a], 15);
        expect(circle.length).toBeGreaterThanOrEqual(24);
        for (const point of circle) expect(Math.hypot(point.x - a.x, point.y - a.y)).toBeCloseTo(15, 6);
    });
});
