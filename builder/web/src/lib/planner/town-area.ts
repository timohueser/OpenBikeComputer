// The outline of the area of a town on the map, from the screen points of its places.

/** A point on the screen, in px. */
export interface Point { x: number; y: number }

/** The corners of the convex hull of `points`, in one turning direction; points on an edge are no corners. */
export function hull(points: Point[]): Point[] {
    const sorted = [...points].sort((a, b) => a.x - b.x || a.y - b.y).filter((p, i, all) => !i || p.x !== all[i - 1].x || p.y !== all[i - 1].y);
    if (sorted.length < 2) return sorted;
    const cross = (o: Point, a: Point, b: Point) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    const half = (list: Point[]) => {
        const chain: Point[] = [];
        for (const point of list) {
            while (chain.length >= 2 && cross(chain.at(-2)!, chain.at(-1)!, point) <= 0) chain.pop();
            chain.push(point);
        }
        return chain.slice(0, -1);
    };
    return [...half(sorted), ...half(sorted.reverse())];
}

/** The west and east ends of the ring `ring` at the height `y`. */
export function span(ring: Point[], y: number): [number, number] {
    const xs = ring.flatMap((a, i) => {
        const b = ring[(i + 1) % ring.length];
        return (a.y - y) * (b.y - y) <= 0 && a.y !== b.y ? [a.x + (b.x - a.x) * (y - a.y) / (b.y - a.y)] : [];
    });
    return [Math.min(...xs), Math.max(...xs)];
}

// The angle between two points of a round corner.
const step = .2;

/** The hull of `points` grown by `radius` with round corners, as a ring of points: two points give a capsule, one a
 * circle. */
export function grownHull(points: Point[], radius: number): Point[] {
    const corners = hull(points), n = corners.length;
    // The direction away from the hull at the edge that leaves corner `i`.
    const outward = (i: number) => {
        const a = corners[i], b = corners[(i + 1) % n];
        return Math.atan2(a.x - b.x, b.y - a.y);
    };
    // The arc of a corner turns from the edge that arrives to the edge that leaves; the arc of one point is the circle.
    return corners.flatMap((corner, i) => {
        const from = n > 1 ? outward((i + n - 1) % n) : 0;
        const turn = n > 1 ? ((outward(i) - from) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) : 2 * Math.PI;
        const steps = Math.ceil(turn / step);
        return Array.from({ length: n > 1 ? steps + 1 : steps }, (_, k) => {
            const angle = from + turn * k / steps;
            return { x: corner.x + radius * Math.cos(angle), y: corner.y + radius * Math.sin(angle) };
        });
    });
}
