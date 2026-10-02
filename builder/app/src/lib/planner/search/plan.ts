import type { Coordinate } from '../editor';

/** The route line a search request carries. Each kept point keeps its kilometre and riding
 *  hour on the full line, so route positions do not move when points drop out. */
export function searchLine(coordinates: Coordinate[], km: number[], seconds?: number[]) {
    // A larger tolerance cuts the corners of hairpins, and places beside them move along the route.
    const kept = simplify(coordinates, .01);
    return {
        coordinates: kept.map(i => coordinates[i].map(v => round(v, 5)) as Coordinate),
        km: kept.map(i => round(km[i], 3)),
        hours: seconds && kept.map(i => round(seconds[i] / 3600, 3)),
    };
}

const round = (value: number, digits: number) => Math.round(value * 10 ** digits) / 10 ** digits;

/** Douglas–Peucker: no dropped point is more than `tolerance` km from the kept line. */
function simplify(line: Coordinate[], tolerance: number): number[] {
    const keep = new Uint8Array(line.length);
    keep[0] = keep[line.length - 1] = 1;
    const spans = line.length > 2 ? [[0, line.length - 1]] : [];
    while (spans.length) {
        const [a, b] = spans.pop()!;
        // Squared distances in latitude degrees, with longitude scaled at the span start.
        const [ax, ay] = line[a], k = Math.cos(ay * Math.PI / 180);
        const dx = (line[b][0] - ax) * k, dy = line[b][1] - ay, length = dx * dx + dy * dy || 1;
        let far = (tolerance / 111.195) ** 2, split = 0;
        for (let i = a + 1; i < b; i++) {
            const px = (line[i][0] - ax) * k, py = line[i][1] - ay;
            const t = Math.max(0, Math.min(1, (px * dx + py * dy) / length));
            const d = (px - t * dx) ** 2 + (py - t * dy) ** 2;
            if (d > far) { far = d; split = i; }
        }
        if (split) { keep[split] = 1; spans.push([a, split], [split, b]); }
    }
    return line.flatMap((_, i) => keep[i] ? [i] : []);
}
