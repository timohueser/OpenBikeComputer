/** The items of the Routes and Explore pages, the chips of their search, and the filter that reads both. */
import type { Bike, RideRecord, RideSummary, RouteRecord, RouteSummary, Summary } from '../../lib/library/records';
import { kilometres, type Coordinate } from '../../lib/planner/geo';
import { decodeCoordinates } from '../../lib/planner/route-answer';
import type { Box } from '../../lib/planner/route-catalog';
import type { CatalogRecord, TrailEntry } from '../../lib/planner/signed-routes';
import { distanceIn, heightIn } from '../../lib/settings/format';
import { profileEntry, profileHeight } from '../../lib/trailview/route/profile';

export type { Box };

/** A route, a ride, a signed route of the catalog or a trail of its far files as the list, the map and the card show
 * it. A route with no summary has no line, and each chip leaves it out. `at` is the time in ms of the last change of a
 * route, or of the start of a ride. */
export type Item = { id: string; name: string; at: number }
    & ({ kind: 'route'; summary: RouteSummary | null } | { kind: 'ride'; summary: RideSummary }
        | { kind: 'signed'; summary: RouteSummary; record: CatalogRecord; profile?: Pick<Summary, 'line' | 'heights'> }
        | { kind: 'trail'; summary: RouteSummary; entry: TrailEntry; pieces: Coordinate[][] });
export type RideItem = Extract<Item, { kind: 'ride' }>;
export type SignedItem = Extract<Item, { kind: 'signed' }>;
export type TrailItem = Extract<Item, { kind: 'trail' }>;

/** From `min` to below `max`, in the units of `settings.units`: km or mi for a length, m or ft for a climb. */
export type Range = [min: number, max: number];

/** "Under 100 km", "100 km or more", "100 km to 200 km". */
export function rangeText([min, max]: Range, format: (value: number) => string): string {
    return min <= 0 ? `Under ${format(max)}` : max === Infinity ? `${format(min)} or more` : `${format(min)} to ${format(max)}`;
}

/** The count of steps of a range slider. */
export const STEPS = 16;

/** The step of a range slider for `values`: the smallest of 1, 2, 5, 10, 20, 50, … whose `STEPS` steps reach above
 * the value at 90 % of the sorted values, so that a few very long routes do not stretch the scale, and above each
 * finite end of `range`, so that a range that is set is inside it. `top` is `STEPS × step`. */
export function scale(values: number[], range?: Range): { step: number; top: number } {
    const sorted = [...values].sort((a, b) => a - b);
    const ends = (range ?? []).filter(Number.isFinite);
    const reach = Math.max(0, sorted[Math.ceil(sorted.length * 9 / 10) - 1] ?? 0, ...ends);
    for (let power = 1; ; power *= 10)
        for (const step of [power, 2 * power, 5 * power]) if (STEPS * step > reach) return { step, top: STEPS * step };
}

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
    /** Signed routes only: the hardest grade that the rider accepts, as an index of `grades_m`. */
    grade?: number;
    /** Signed routes only. */
    shape?: 'loop' | 'one-way';
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

/** `profile_m` of a catalog record has a height at each PROFILE_STEP_M metres along the route, and one at its end. */
export const PROFILE_STEP_M = 100;

const round5 = (value: number) => Math.round(value * 1e5) / 1e5;

/** A route record of the catalog. The height of a point of its line is the profile at the share of the line's length
 * that the point has (`specs/route-catalog.md`, "Profile"). With a profile, `profile` places each entry on the line at
 * the share of the route's length that the entry has. */
export function signedItem(record: CatalogRecord, bike: Bike): SignedItem {
    const points = decodeCoordinates(record.line_udeg ?? []), along = [0];
    for (let i = 1; i < points.length; i++) along.push(along[i - 1] + kilometres(points[i - 1], points[i]));
    const total = along.at(-1)!, entries = record.profile_m, length = record.length_m;
    const profile = entries && { step: PROFILE_STEP_M, heights: entries, length };
    const heightAt = (share: number): number | null => {
        const height = profile ? profileHeight(profile, share * length) : null;
        return height === null ? null : Math.round(height);
    };
    /** The point of the line at a share of its length; `from` is the segment to search from. */
    let from = 0;
    const pointAt = (share: number): Coordinate => {
        const at = share * total;
        while (from < along.length - 2 && along[from + 1] < at) from++;
        const span = along[from + 1] - along[from], t = span > 0 ? Math.max(0, Math.min(1, (at - along[from]) / span)) : 0;
        const [a, b] = [points[from], points[from + 1] ?? points[from]];
        return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    };
    const lons = points.map(point => point[0]), lats = points.map(point => point[1]);
    const known = entries?.filter((entry): entry is number => entry !== null) ?? [];
    return {
        kind: 'signed', id: String(record.id), name: record.name ?? record.ref ?? '', at: 0, record,
        summary: {
            line: points.flatMap(([lon, lat]) => [round5(lon), round5(lat)]),
            heights: along.map(metres => heightAt(total > 0 ? metres / total : 0)),
            bounds: [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)],
            lengthM: length, climbM: record.ascent_m, highestM: known.length ? Math.max(...known) : null, seconds: 0, bike, days: 1,
        },
        ...entries && points.length > 1
            ? { profile: { line: entries.flatMap((_, i) => pointAt(length > 0 ? profileEntry(profile!, i) / length : 0).map(round5)), heights: entries } } : {},
    };
}

/** A trail of the far files, with the pieces of its far line. Its summary line is the pieces joined, with no heights. */
export function trailItem(entry: TrailEntry, bike: Bike): TrailItem {
    const pieces = entry.far_udeg.map(decodeCoordinates), line = pieces.flat().flatMap(([lon, lat]) => [round5(lon), round5(lat)]);
    const lons = line.filter((_, i) => i % 2 === 0), lats = line.filter((_, i) => i % 2 === 1);
    return {
        kind: 'trail', id: String(entry.id), name: entry.name ?? entry.ref ?? '', at: 0, entry, pieces,
        summary: {
            line, heights: lons.map(() => null), bounds: [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)],
            lengthM: entry.length_m, climbM: entry.ascent_m, highestM: null, seconds: 0, bike, days: 1,
        },
    };
}

/** The order of the far list: the highest network level first, then the longest, then the lowest relation id. */
export const farOrder = (a: TrailItem, b: TrailItem) => b.entry.rank - a.entry.rank || b.entry.length_m - a.entry.length_m || a.entry.id - b.entry.id;

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
 * to it comes first. Else, and for a tie, `newest` puts the last changed first, and `shortest` the shortest and then the
 * lowest relation id. */
export function results(items: Item[], chips: Chips, text: string, view?: Box, order: 'newest' | 'shortest' = 'newest'): Item[] {
    const words = fold(text.trim()), set = Object.values(chips).some(chip => chip !== undefined);
    const kept: { item: Item; near: number }[] = [];
    for (const item of items) {
        if (words && !fold(item.name).includes(words)) continue;
        const summary = item.summary, signed = item.kind === 'signed' ? item.record : item.kind === 'trail' ? item.entry : undefined;
        if (!summary) {
            if (!set && !view) kept.push({ item, near: 0 });
            continue;
        }
        if ((chips.bike && summary.bike !== chips.bike)
            || (chips.length && !within(distanceIn(summary.lengthM), chips.length))
            || (chips.climb && !within(heightIn(summary.climbM), chips.climb))
            || (chips.days && !within('days' in summary ? summary.days : 1, chips.days))
            || (chips.year && new Date(item.at).getFullYear() !== chips.year)
            || (signed && chips.grade !== undefined && (signed.hardest ?? 0) > chips.grade)
            || (signed && chips.shape && signed.loop !== (chips.shape === 'loop'))
            || (view && !inBox(summary.line, view))) continue;
        const near = chips.place ? distanceToBox(summary.line, chips.place.box) : 0;
        if (chips.place && near > (NEAR_M[chips.place.kind] ?? OTHER_NEAR_M)) continue;
        kept.push({ item, near });
    }
    const tie = order === 'shortest'
        ? (a: Item, b: Item) => (a.summary?.lengthM ?? 0) - (b.summary?.lengthM ?? 0) || Number(a.id) - Number(b.id)
        : (a: Item, b: Item) => b.at - a.at;
    return kept.sort((a, b) => a.near - b.near || tie(a.item, b.item)).map(({ item }) => item);
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
