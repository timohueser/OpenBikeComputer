/** A route is under the pointer when a segment between two of its pick points passes this near, in CSS pixels. */
const REACH_PX = 12;

/**
 * The number of the route under the pointer at (x, y), or null for none. Entry k is route k + 1: x and y
 * in CSS pixels and the distance from the eye of each pick point, NaN behind the eye, where a segment
 * ends. Of the routes with a segment within REACH_PX, the bold route wins, then the route whose nearest
 * place on such a segment is nearest to the eye, then the lower number.
 */
export function nearestRoute(routes: readonly (Float32Array | undefined)[], x: number, y: number, bold: number | null): number | null {
    let best: { route: number; distance: number } | undefined;
    routes.forEach((points, k) => {
        const distance = points && nearest(points, x, y);
        if (distance === undefined) return;
        if (k + 1 === bold) best = { route: bold, distance: -Infinity };
        else if (!best || distance < best.distance) best = { route: k + 1, distance };
    });
    return best?.route ?? null;
}

/** The least distance from the eye of the nearest places on the segments within REACH_PX of (x, y), or undefined. */
function nearest(points: Float32Array, x: number, y: number): number | undefined {
    let least: number | undefined;
    for (let i = 3; i < points.length; i += 3) {
        const ax = points[i - 3], ay = points[i - 2], dx = points[i] - ax, dy = points[i + 1] - ay;
        const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
        // A point behind the eye makes every comparison false.
        if (!(Math.hypot(ax + t * dx - x, ay + t * dy - y) <= REACH_PX)) continue;
        const distance = points[i - 1] + t * (points[i + 2] - points[i - 1]);
        if (least === undefined || distance < least) least = distance;
    }
    return least;
}
