import { readGpx } from '../coverage/gpx';
import { emptyTrip, kilometres, maxRidingDays, nearestProgress, type Coordinate, type DrawnCoordinate, type RoutePoint, type Trip } from './editor';
import type { Shape } from './routing';

/** One imported file: its name, its simplified line with the file's elevations, its waypoints and, when planned on roads,
 * its shape. */
export interface ImportedLine {
    name: string;
    line: DrawnCoordinate[];
    waypoints: { label: string; coordinate: Coordinate }[];
    shape?: Shape;
}

/** Chosen: a few metres drops recording noise and keeps the line visually exact. */
const toleranceM = 3;
/** Two days whose ends are farther apart than this are joined by a transfer. */
const transferMinKm = 0.2;

/** Douglas–Peucker in a local plane: every dropped point is within `tolerance` metres of the kept line. */
export function simplifyLine<T extends DrawnCoordinate>(line: T[], tolerance = toleranceM): T[] {
    if (line.length < 3) return line;
    const scale = Math.cos(line[0][1] * Math.PI / 180);
    const offset = (p: T, a: T, b: T) => {
        const [ax, ay] = [(p[0] - a[0]) * scale, p[1] - a[1]];
        const [dx, dy] = [(b[0] - a[0]) * scale, b[1] - a[1]];
        const t = Math.max(0, Math.min(1, (ax * dx + ay * dy) / (dx * dx + dy * dy || 1)));
        return Math.hypot(ax - dx * t, ay - dy * t) * 111_320;
    };
    const keep = new Uint8Array(line.length);
    keep[0] = keep[line.length - 1] = 1;
    const ranges: [number, number][] = [[0, line.length - 1]];
    while (ranges.length) {
        const [from, to] = ranges.pop()!;
        let worst = 0, index = 0;
        for (let i = from + 1; i < to; i++) {
            const distance = offset(line[i], line[from], line[to]);
            if (distance > worst) { worst = distance; index = i; }
        }
        if (worst <= tolerance) continue;
        keep[index] = 1;
        ranges.push([from, index], [index, to]);
    }
    return line.filter((_, i) => keep[i]);
}

/** One day per file, in the given order. */
export function readTracks(files: { name: string; text: string }[]): ImportedLine[] {
    if (files.length > maxRidingDays) throw new Error(`A trip has at most ${maxRidingDays} days. Import ${maxRidingDays} files or fewer.`);
    return files.map(file => {
        try {
            const { name, points, waypoints } = readGpx(file.text, file.name.replace(/\.gpx$/i, ''));
            return { name, line: simplifyLine(points.map((p): DrawnCoordinate => p.ele === undefined ? [p.lon / 1e6, p.lat / 1e6] : [p.lon / 1e6, p.lat / 1e6, p.ele])),
                waypoints: waypoints.map(w => ({ label: w.name ?? 'Marker', coordinate: [w.lon / 1e6, w.lat / 1e6] })) };
        } catch (error) {
            throw new Error(`${file.name}: ${(error as Error).message}`);
        }
    });
}

/** Shapes the lines one at a time, so an import never sends parallel requests. Any failure, such as a busy service, keeps
 * the file's line and names the file in `failed`. */
export async function planOnRoads(lines: ImportedLine[], shape: (line: Coordinate[]) => Promise<Shape>): Promise<{ lines: ImportedLine[]; failed: string[] }> {
    const shaped: ImportedLine[] = [], failed: string[] = [];
    for (const line of lines) {
        try { shaped.push({ ...line, shape: await shape(line.line.map(c => [c[0], c[1]])) }); }
        catch { shaped.push(line); failed.push(line.name); }
    }
    return { lines: shaped, failed };
}

/**
 * One file is a route; several are a trip with one day per file and a night at each day end. A kept line is one drawn
 * leg that repeats its ends, so their elevations stay; a day that continues from the previous day end also repeats that
 * end. A shaped line is routed through its shape points. A day that starts more than 200 m from the previous day end
 * starts with a transfer; a nearer day continues from that day end. The files' waypoints are markers, never route points.
 */
export function importedTrip(base: Pick<Trip, 'bike' | 'preset'>, lines: ImportedLine[]): Trip {
    const points: RoutePoint[] = [];
    const add = (kind: RoutePoint['kind'], coordinate: Coordinate, extra: Partial<RoutePoint> = {}) =>
        points.push({ id: crypto.randomUUID(), kind, label: kind === 'start' ? 'Start' : kind === 'finish' ? 'Finish' : kind === 'night' ? 'Overnight spot' : 'Shaping point',
            coordinate, progress: 0, ...extra });
    lines.forEach(({ line, shape }, day) => {
        const before = points.at(-1);
        const previous = lines[day - 1]?.line.at(-1);
        const [first, final] = [line[0], line.at(-1)!].map((c): Coordinate => [c[0], c[1]]);
        const joined = !!before && kilometres(before.coordinate, first) <= transferMinKm;
        if (!before) add('start', first);
        else if (!joined) add('via', first, { leg: 'transfer' });
        const stops = shape?.points ?? [first, final];
        for (let i = 1; i < stops.length; i++) {
            const last = i === stops.length - 1;
            const kind = !last ? 'via' : day === lines.length - 1 ? 'finish' : 'night';
            add(kind, stops[i], {
                ...kind === 'night' ? { id: `night-${day + 1}`, night: day + 1 } : {},
                ...shape?.turnarounds.includes(i) ? { turnaround: true as const } : {},
                ...shape ? {} : { leg: 'drawn' as const, drawn: joined && !lines[day - 1].shape ? [previous!, ...line] : line },
            });
        }
    });
    points.forEach((point, i) => point.progress = i / (points.length - 1));
    // A marker stays in its own file's day; one array per line lets `cumulative` measure it once. A zero-length line has no
    // progress (NaN), so its markers sit mid-day.
    const markers = lines.flatMap(({ line, waypoints }, file) => {
        const flat = line.map((c): Coordinate => [c[0], c[1]]);
        return waypoints.map(({ label, coordinate }): RoutePoint => ({ id: crypto.randomUUID(), kind: 'marker', label, coordinate,
            progress: (file + (nearestProgress(flat, coordinate) || .5)) / lines.length }));
    });
    const days = lines.length > 1 ? lines.length : undefined;
    return { ...emptyTrip(days ? 'trip' : 'route'), bike: base.bike, preset: base.preset, points: [...points, ...markers],
        routeOrder: points.slice(1, -1).map(point => point.id), ...days ? { days, target: days } : {} };
}
