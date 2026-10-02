import type { Coordinate } from '../editor';

type SearchLine = { coordinates: Coordinate[]; km: number[]; hours?: number[] };
const lines = new WeakMap<Coordinate[], { seconds?: number[]; line: SearchLine }>();

/** The route line a search request carries. Each kept point keeps its kilometre and riding
 *  hour on the full line, so route positions do not move when points drop out. A route's
 *  line is computed at its first query and reused until the route changes. */
export function searchLine(coordinates: Coordinate[], km: number[], seconds?: number[]): SearchLine {
    const cached = lines.get(coordinates);
    if (cached && cached.seconds === seconds) return cached.line;
    // The route lives in deep reactive state, where each read is slow: read it once.
    const plain = coordinates.map(c => [c[0], c[1]] as Coordinate), time = seconds?.slice();
    const kept = simplify(plain, km, time);
    const line = {
        coordinates: kept.map(i => plain[i].map(v => round(v, 5)) as Coordinate),
        // The last kilometre stays exact: the planner sends the last day's end as the route length.
        km: kept.map(i => i === plain.length - 1 ? km[i] : round(km[i], 3)),
        hours: time && kept.map(i => round(time[i] / 3600, 3)),
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

/** Douglas–Peucker over two errors: the distance from the kept line, and the riding time
 *  against the time interpolated by kilometre between kept points. */
function simplify(line: Coordinate[], km: number[], seconds?: number[]): number[] {
    const keep = new Uint8Array(line.length);
    keep[0] = keep[line.length - 1] = 1;
    const spans = line.length > 2 ? [[0, line.length - 1]] : [];
    const offset = (OFFSET_KM / 111.195) ** 2;
    while (spans.length) {
        const [a, b] = spans.pop()!;
        // Squared distances in latitude degrees, with longitude scaled at the span start.
        const [ax, ay] = line[a], k = Math.cos(ay * Math.PI / 180);
        const dx = (line[b][0] - ax) * k, dy = line[b][1] - ay, length = dx * dx + dy * dy || 1;
        const pace = seconds ? (seconds[b] - seconds[a]) / (km[b] - km[a] || 1) : 0;
        let worst = 1, split = 0;
        for (let i = a + 1; i < b; i++) {
            const px = (line[i][0] - ax) * k, py = line[i][1] - ay;
            const t = Math.max(0, Math.min(1, (px * dx + py * dy) / length));
            const off = ((px - t * dx) ** 2 + (py - t * dy) ** 2) / offset;
            const late = seconds ? ((seconds[i] - seconds[a] - pace * (km[i] - km[a])) / DELAY_SECONDS) ** 2 : 0;
            if (Math.max(off, late) > worst) { worst = Math.max(off, late); split = i; }
        }
        if (split) { keep[split] = 1; spans.push([a, split], [split, b]); }
    }
    return line.flatMap((_, i) => keep[i] ? [i] : []);
}
