/** Longitude and latitude in degrees. */
export type Coordinate = [number, number];
/** A longitude and latitude, perhaps followed by an elevation. */
type Point = readonly number[];

const RAD = Math.PI / 180;
/** Kilometres per degree of latitude on the sphere of `kilometres`. */
export const kmPerDegree = 6371 * RAD;

export function kilometres(a: Point, b: Point): number {
    const x = Math.sin((b[1] - a[1]) * RAD / 2) ** 2
        + Math.cos(a[1] * RAD) * Math.cos(b[1] * RAD) * Math.sin((b[0] - a[0]) * RAD / 2) ** 2;
    return 6371 * 2 * Math.atan2(Math.sqrt(x), Math.sqrt(1 - x));
}

// Coordinate arrays are never changed after they are built, so each array's distances are computed once.
// Development and test builds freeze a measured array and its pairs, so an in-place edit throws.
const distances = new WeakMap<Coordinate[], readonly number[]>();

/** Kilometres from the first coordinate to each coordinate. */
export function cumulative(coordinates: Coordinate[]): readonly number[] {
    const known = distances.get(coordinates);
    if (known) return known;
    if (import.meta.env.DEV) {
        for (const pair of coordinates) Object.freeze(pair);
        Object.freeze(coordinates);
    }
    const result = [0];
    for (let i = 1; i < coordinates.length; i++) result.push(result[i - 1] + kilometres(coordinates[i - 1], coordinates[i]));
    distances.set(coordinates, result);
    return result;
}

/** The first index at which `reached` is true, or `length`; once true, `reached` must stay true. */
export function firstIndex(length: number, reached: (index: number) => boolean): number {
    let low = 0, high = length;
    while (low < high) {
        const middle = (low + high) >>> 1;
        if (reached(middle)) high = middle;
        else low = middle + 1;
    }
    return low;
}

export function coordinateAt(coordinates: Coordinate[], progress: number): Coordinate {
    if (coordinates.length === 0) throw new Error('A route needs at least one coordinate.');
    if (coordinates.length === 1) return [...coordinates[0]];
    const distances = cumulative(coordinates);
    const goal = distances.at(-1)! * Math.max(0, Math.min(1, progress));
    const index = Math.max(1, Math.min(distances.length - 1, firstIndex(distances.length, i => distances[i] >= goal)));
    const t = (goal - distances[index - 1]) / (distances[index] - distances[index - 1] || 1);
    return coordinates[index - 1].map((n, axis) => n + (coordinates[index][axis] - n) * t) as Coordinate;
}

export function routeSlice(coordinates: Coordinate[], from: number, to: number): Coordinate[] {
    if (coordinates.length === 0) return [];
    if (from > to) return routeSlice(coordinates, to, from).reverse();
    const start = Math.max(0, Math.min(1, from));
    const end = Math.max(0, Math.min(1, to));
    const first = coordinateAt(coordinates, start);
    if (start === end || coordinates.length === 1) return [first];
    const distances = cumulative(coordinates);
    const total = distances.at(-1)!;
    const inside = coordinates.slice(
        firstIndex(distances.length, i => distances[i] > start * total),
        firstIndex(distances.length, i => distances[i] >= end * total),
    );
    return [first, ...inside, coordinateAt(coordinates, end)];
}

// The fraction along the segment from `a` to `a + d` nearest the origin, and its squared distance, in a flat plane.
function project(ax: number, ay: number, dx: number, dy: number): { t: number; squared: number } {
    const t = Math.max(0, Math.min(1, -(ax * dx + ay * dy) / (dx * dx + dy * dy || 1)));
    return { t, squared: (ax + t * dx) ** 2 + (ay + t * dy) ** 2 };
}

/**
 * The position on `line` nearest `point`: the segment by its first vertex, the fraction along it, and the coordinate.
 * Distances are flat kilometres around the point, which is exact enough at route scale. Of positions equally near, such
 * as on a stretch that the route rides out and back, the first wins; the margin on squared kilometres absorbs float rounding.
 */
export function nearestOnLine(line: readonly Point[], point: Point): { index: number; t: number; at: Coordinate } {
    const kx = kmPerDegree * Math.cos(point[1] * RAD);
    let index = 0, t = 0, nearest = Infinity;
    for (let i = 1; i < line.length; i++) {
        const a = line[i - 1], b = line[i];
        const found = project((a[0] - point[0]) * kx, (a[1] - point[1]) * kmPerDegree, (b[0] - a[0]) * kx, (b[1] - a[1]) * kmPerDegree);
        if (found.squared < nearest - 1e-9) { nearest = found.squared; t = found.t; index = i - 1; }
    }
    const a = line[index], b = line[index + 1] ?? a;
    return { index, t, at: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t] };
}

/** Progress from 0 to 1 along the line to the position nearest `point`. On a line without length every position is the middle. */
export function nearestProgress(coordinates: Coordinate[], point: Point): number {
    const lengths = cumulative(coordinates), total = lengths.at(-1);
    if (!total) return .5;
    const { index, t } = nearestOnLine(coordinates, point);
    return (lengths[index] + (lengths[index + 1] - lengths[index]) * t) / total;
}

/**
 * Distance in km from a point to the line, or Infinity beyond `km`. Each piece of the line has a bounding box, so a point
 * skips far pieces at once. A one-point line is its point.
 */
export function routeDistance(coordinates: Coordinate[], km: number): (point: Point) => number {
    const pieces: { line: Coordinate[]; box: number[] }[] = [];
    for (let i = 0; i < coordinates.length; i += 64) {
        const line = coordinates.slice(Math.max(0, i - 1), i + 64);
        const lons = line.map(c => c[0]), lats = line.map(c => c[1]);
        pieces.push({ line, box: [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)] });
    }
    return ([lon, lat]) => {
        const kx = kmPerDegree * Math.cos(lat * RAD);
        const dLon = km / kx, dLat = km / kmPerDegree;
        let nearest = Infinity;
        for (const { line, box } of pieces) {
            if (lon < box[0] - dLon || box[2] + dLon < lon || lat < box[1] - dLat || box[3] + dLat < lat) continue;
            line.forEach((b, i) => {
                const a = line[Math.max(0, i - 1)];
                nearest = Math.min(nearest, project((a[0] - lon) * kx, (a[1] - lat) * kmPerDegree, (b[0] - a[0]) * kx, (b[1] - a[1]) * kmPerDegree).squared);
            });
        }
        return nearest <= km * km ? Math.sqrt(nearest) : Infinity;
    };
}

/**
 * Douglas–Peucker in a flat plane: the indices of the points to keep, first and last included. Each dropped point lies
 * within `toleranceKm` of the kept line, and `error(i, a, b)`, when given, is at most 1 for it between the kept points `a` and `b`.
 */
export function simplify(line: readonly Point[], toleranceKm: number, error?: (i: number, a: number, b: number) => number): number[] {
    const keep = new Uint8Array(line.length);
    keep[0] = keep[line.length - 1] = 1;
    const spans = line.length > 2 ? [[0, line.length - 1]] : [];
    while (spans.length) {
        const [a, b] = spans.pop()!;
        const kx = kmPerDegree * Math.cos(line[a][1] * RAD);
        const dx = (line[b][0] - line[a][0]) * kx, dy = (line[b][1] - line[a][1]) * kmPerDegree;
        let worst = 1, split = 0;
        for (let i = a + 1; i < b; i++) {
            const off = project((line[a][0] - line[i][0]) * kx, (line[a][1] - line[i][1]) * kmPerDegree, dx, dy).squared / toleranceKm ** 2;
            const value = error ? Math.max(off, error(i, a, b) ** 2) : off;
            if (value > worst) { worst = value; split = i; }
        }
        if (split) { keep[split] = 1; spans.push([a, split], [split, b]); }
    }
    return line.flatMap((_, i) => keep[i] ? [i] : []);
}
