/** The items of the Routes page, the chips of its search, and the filter that reads both. */
import type { Bike, RideRecord, RideSummary, RouteRecord, RouteSummary } from '../../lib/library/records';
import { distanceIn, heightIn } from '../../lib/settings/format';

/** West, south, east, north. */
export type Box = [number, number, number, number];

/** A route or a ride as the list, the map and the card show it. A route with no summary has no line, and each chip
 * leaves it out. `at` is the time in ms of the last change of a route, or of the start of a ride. */
export type Item = { id: string; name: string; at: number }
    & ({ kind: 'route'; summary: RouteSummary | null } | { kind: 'ride'; summary: RideSummary });
export type RideItem = Extract<Item, { kind: 'ride' }>;

/** From `min` to below `max`, in the units of `settings.units`: km or mi for a length, m or ft for a climb. */
export type Range = [min: number, max: number];

/** A place of the place search. A place with no extent has its point as its box. */
export interface Place { name: string; kind: string; box: Box }

/** The chips of the search. An item must match each chip that is set. */
export interface Chips {
    bike?: Bike;
    place?: Place;
    length?: Range;
    climb?: Range;
    days?: Range;
    /** The local year of `at`. */
    year?: number;
}

/** The planner functions that name a plan. They reach the map catalogue, so the page loads them after its list. */
export interface Naming {
    isPlan: (route: RouteRecord) => boolean;
    planTitle: (trip: never) => string;
}

/** A plan with no name has the title of its plan. A route that is not a plan has no line. Without `naming`, a route with
 * no name has the names of its start and its finish. */
export function routeItem(route: RouteRecord, naming?: Naming): Item {
    const { id, name, summary, updatedAt: at } = route;
    if (!naming) return { kind: 'route', id, name: name || endNames(route.trip), summary, at };
    if (!naming.isPlan(route)) return { kind: 'route', id, name: name || 'Route that cannot be read', summary: null, at };
    return { kind: 'route', id, name: name || naming.planTitle(route.trip as never), summary, at };
}

export const rideItem = (ride: RideRecord): RideItem =>
    ({ kind: 'ride', id: ride.key, name: ride.name, summary: ride.summary, at: ride.summary.start * 1000 });

/** "Start → Finish" from the points of a trip that no check read yet, else "Route". */
function endNames(trip: unknown): string {
    const points = (trip as { points?: unknown } | null)?.points;
    const label = (kind: string) => Array.isArray(points)
        ? points.find(point => point?.kind === kind && typeof point.label === 'string' && point.label)?.label as string | undefined : undefined;
    return [label('start'), label('finish')].filter(Boolean).join(' → ') || 'Route';
}

/** The distance from a place within which a line matches it, by the kind of the place. */
const NEAR_M: Record<string, number> = { hamlet: 3000, village: 3000, suburb: 3000, district: 3000, locality: 3000, town: 5000, city: 10_000 };
const OTHER_NEAR_M = 2000;
const METRES_PER_DEGREE = 6_371_008.8 * Math.PI / 180;

const fold = (text: string) => text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
const within = (value: number, [min, max]: Range) => value >= min && value < max;

/** The items that match the text and the chips, and with `view` that have a point in it. With a place, the line nearest
 * to it comes first; else, and for a tie, the last changed. */
export function results(items: Item[], chips: Chips, text: string, view?: Box): Item[] {
    const words = fold(text.trim()), set = Object.values(chips).some(chip => chip !== undefined);
    const kept: { item: Item; near: number }[] = [];
    for (const item of items) {
        if (words && !fold(item.name).includes(words)) continue;
        const summary = item.summary;
        if (!summary) {
            if (!set && !view) kept.push({ item, near: 0 });
            continue;
        }
        if ((chips.bike && summary.bike !== chips.bike)
            || (chips.length && !within(distanceIn(summary.lengthM), chips.length))
            || (chips.climb && !within(heightIn(summary.climbM), chips.climb))
            || (chips.days && !within('days' in summary ? summary.days : 1, chips.days))
            || (chips.year && new Date(item.at).getFullYear() !== chips.year)
            || (view && !inBox(summary.line, view))) continue;
        const near = chips.place ? distanceToBox(summary.line, chips.place.box) : 0;
        if (chips.place && near > (NEAR_M[chips.place.kind] ?? OTHER_NEAR_M)) continue;
        kept.push({ item, near });
    }
    return kept.sort((a, b) => a.near - b.near || b.item.at - a.item.at).map(({ item }) => item);
}

function inBox(line: number[], [west, south, east, north]: Box): boolean {
    for (let i = 0; i < line.length; i += 2) if (line[i] >= west && line[i] <= east && line[i + 1] >= south && line[i + 1] <= north) return true;
    return false;
}

/** The distance in metres from a line (longitude, latitude, …) to a box, on a flat projection at the latitude of the box. */
export function distanceToBox(line: number[], box: Box): number {
    const kx = Math.cos((box[1] + box[3]) / 2 * Math.PI / 180) * METRES_PER_DEGREE, ky = METRES_PER_DEGREE;
    const [x0, y0, x1, y1] = [box[0] * kx, box[1] * ky, box[2] * kx, box[3] * ky];
    const toBox = (x: number, y: number) => Math.hypot(Math.max(x0 - x, 0, x - x1), Math.max(y0 - y, 0, y - y1));
    let best = toBox(line[0] * kx, line[1] * ky);
    for (let i = 2; i < line.length; i += 2) {
        const ax = line[i - 2] * kx, ay = line[i - 1] * ky, bx = line[i] * kx, by = line[i + 1] * ky;
        if (crosses(ax, ay, bx, by, x0, y0, x1, y1)) return 0;
        // A segment that does not cross the box is nearest to it at one of its ends or at a corner of the box.
        best = Math.min(best, toBox(bx, by), toSegment(x0, y0, ax, ay, bx, by), toSegment(x1, y0, ax, ay, bx, by),
            toSegment(x0, y1, ax, ay, bx, by), toSegment(x1, y1, ax, ay, bx, by));
    }
    return best;
}

/** Liang–Barsky: the segment has a part inside the box. */
function crosses(ax: number, ay: number, bx: number, by: number, x0: number, y0: number, x1: number, y1: number): boolean {
    const dx = bx - ax, dy = by - ay;
    let from = 0, to = 1;
    for (const [p, q] of [[-dx, ax - x0], [dx, x1 - ax], [-dy, ay - y0], [dy, y1 - ay]]) {
        if (p === 0) {
            if (q < 0) return false;
            continue;
        }
        const t = q / p;
        if (p < 0) from = Math.max(from, t);
        else to = Math.min(to, t);
        if (from > to) return false;
    }
    return true;
}

function toSegment(x: number, y: number, ax: number, ay: number, bx: number, by: number): number {
    const dx = bx - ax, dy = by - ay, length = dx * dx + dy * dy;
    const t = length ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / length)) : 0;
    return Math.hypot(x - ax - t * dx, y - ay - t * dy);
}

/** A place of the search, with its point and its region for the menu. */
export interface FoundPlace extends Place { lon: number; lat: number; region: string }

/** The places that the place search finds for `text` around `view`. None when the search reads the text as another kind
 * of request. */
export async function findPlaces(text: string, view: Box, signal: AbortSignal): Promise<FoundPlace[]> {
    const { searchPlaces } = await import('../../lib/planner/search/types');
    const answer = await searchPlaces(text, { view, plan: { coordinates: [], km: [], days: [], points: [] } }, 6, signal);
    if (answer.type !== 'places') return [];
    return (answer.results ?? []).map(({ name, kind, lon, lat, city, region, west, south, east, north }) => ({
        name, kind, lon, lat, region: [city, region].filter(part => part && part !== name).join(', '),
        box: west === undefined || south === undefined || east === undefined || north === undefined ? [lon, lat, lon, lat] : [west, south, east, north],
    }));
}

const isLetter = (char: string | undefined) => !!char && /[\p{L}\p{N}]/u.test(char);

/** Turns the text of the search field into chips and the rest of the text. Today it finds a place that the text names as
 * whole words; a search that understands requests can later fill the same chips. */
export async function readSearch(text: string, view: Box, signal: AbortSignal): Promise<{ chips: Chips; rest: string }> {
    const folded = fold(text);
    for (const { name, kind, box } of await findPlaces(text, view, signal)) {
        const at = name ? folded.indexOf(fold(name)) : -1;
        // `fold` keeps the length of composed text, so `at` is an index of `text` too.
        if (at < 0 || isLetter(folded[at - 1]) || isLetter(folded[at + name.length])) continue;
        return { chips: { place: { name, kind, box } }, rest: `${text.slice(0, at)} ${text.slice(at + name.length)}`.replace(/\s+/g, ' ').trim() };
    }
    return { chips: {}, rest: text.trim() };
}
