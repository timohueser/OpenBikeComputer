import { simplify, type Coordinate } from '../geo';

type SearchLine = { coordinates: Coordinate[]; km: number[]; hours?: number[] };
const lines = new WeakMap<Coordinate[], { seconds?: number[]; line: SearchLine }>();

/** The route line a search request carries. Each kept point keeps its kilometre and riding
 *  hour on the full line, so route positions do not move when points drop out. A route's
 *  line is computed at its first query and reused until the route changes. */
export function searchLine(coordinates: Coordinate[], km: readonly number[], seconds?: number[]): SearchLine {
    const cached = lines.get(coordinates);
    if (cached && cached.seconds === seconds) return cached.line;
    // Douglas–Peucker over two errors: the distance from the kept line, and the riding time against the time
    // interpolated by kilometre between kept points.
    const kept = simplify(coordinates, OFFSET_KM, seconds && ((i, a, b) =>
        (seconds[i] - seconds[a] - (seconds[b] - seconds[a]) / (km[b] - km[a] || 1) * (km[i] - km[a])) / DELAY_SECONDS));
    const line = {
        coordinates: kept.map(i => coordinates[i].map(v => round(v, 5)) as Coordinate),
        // The last kilometre stays exact: the planner sends the last day's end as the route length.
        // No rounded kilometre may pass it, or the kilometres stop rising.
        km: kept.map(i => i === coordinates.length - 1 ? km[i] : Math.min(round(km[i], 3), km[coordinates.length - 1])),
        hours: seconds && kept.map(i => round(seconds[i] / 3600, 3)),
    };
    lines.set(coordinates, { seconds, line });
    return line;
}

const round = (value: number, digits: number) => Math.round(value * 10 ** digits) / 10 ** digits;
// A larger offset cuts the corners of hairpins, and places beside them move along the route.
const OFFSET_KM = .01;
// Search places hour marks between kept points by kilometre. Ten seconds of riding is about as
// far as the offset moves a place along the route.
const DELAY_SECONDS = 10;
